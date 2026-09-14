#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::error::{Error, Result};
use crate::wire::crc::Crc8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GdpAddress(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdpType { Gctl, Gts, Reserved(u8) }
impl GdpType {
    pub const fn to_wire(self) -> u8 { match self { Self::Gctl=>0x1, Self::Gts=>0x2, Self::Reserved(v)=>v&0x0f } }
    pub const fn from_wire(v:u8)->Self { match v&0x0f {1=>Self::Gctl,2=>Self::Gts,x=>Self::Reserved(x)} }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SizeClass {
    Empty=0, Tiny3=1, Ctrl32=2, Ctrl64=3, Msg128=4, Msg192=5, Msg256=6, Msg384=7,
    Medium512=8, Medium768=9, Bulk1K=10, Bulk1280=11, Legacy1500=12, Xmtu2K=13, Jumbo4K=14, Jumbo8K=15,
}
impl SizeClass {
    pub const ALL:[Self;16]=[Self::Empty,Self::Tiny3,Self::Ctrl32,Self::Ctrl64,Self::Msg128,Self::Msg192,Self::Msg256,Self::Msg384,Self::Medium512,Self::Medium768,Self::Bulk1K,Self::Bulk1280,Self::Legacy1500,Self::Xmtu2K,Self::Jumbo4K,Self::Jumbo8K];
    pub const fn bytes(self)->usize{[0,3,32,64,128,192,256,384,512,768,1024,1280,1500,2048,4096,8192][self as usize]}
    pub fn from_wire(v:u8)->Result<Self>{Self::ALL.get((v&0x0f)as usize).copied().ok_or(Error::InvalidSizeClass)}
    pub fn smallest_for(required:usize)->Option<Self>{Self::ALL.into_iter().find(|c|c.bytes()>=required)}
}

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum AddressForm{Global,Local}

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct GdpWireConfig{pub local_form_bit:bool}
impl Default for GdpWireConfig{fn default()->Self{Self{local_form_bit:true}}}
impl GdpWireConfig{
    pub const fn form_bit(self,form:AddressForm)->bool{match form{AddressForm::Local=>self.local_form_bit,AddressForm::Global=>!self.local_form_bit}}
    pub const fn form_from_bit(self,bit:bool)->AddressForm{if bit==self.local_form_bit{AddressForm::Local}else{AddressForm::Global}}
}

#[derive(Debug,Clone,PartialEq,Eq)]
pub enum GdpAddresses{
    Global{destination:GdpAddress,source:GdpAddress},
    Local{destination:u16,source:u16,prefix:u64},
}
impl GdpAddresses{
    pub const fn form(&self)->AddressForm{match self{Self::Global{..}=>AddressForm::Global,Self::Local{..}=>AddressForm::Local}}
    pub const fn effective_destination(&self)->GdpAddress{match *self{Self::Global{destination,..}=>destination,Self::Local{destination,prefix,..}=>GdpAddress((prefix&!0xffff)|destination as u64)}}
    pub const fn effective_source(&self)->GdpAddress{match *self{Self::Global{source,..}=>source,Self::Local{source,prefix,..}=>GdpAddress((prefix&!0xffff)|source as u64)}}
}

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct GdpHeader{
    pub version:u8,
    pub packet_type:GdpType,
    pub size_class:SizeClass,
    pub hop_limit:u8,
    pub addresses:GdpAddresses,
}
impl GdpHeader{
    pub fn global(packet_type:GdpType,size_class:SizeClass,hop_limit:u8,source:GdpAddress,destination:GdpAddress)->Self{Self{version:0,packet_type,size_class,hop_limit,addresses:GdpAddresses::Global{destination,source}}}
    pub fn local(packet_type:GdpType,size_class:SizeClass,hop_limit:u8,prefix:u64,source:u16,destination:u16)->Result<Self>{if hop_limit>15{return Err(Error::InvalidField)}Ok(Self{version:0,packet_type,size_class,hop_limit,addresses:GdpAddresses::Local{destination,source,prefix:prefix&!0xffff}})}
    pub const fn header_len(&self)->usize{match self.addresses{GdpAddresses::Global{..}=>20,GdpAddresses::Local{..}=>8}}
    pub const fn source(&self)->GdpAddress{self.addresses.effective_source()}
    pub const fn destination(&self)->GdpAddress{self.addresses.effective_destination()}
    pub fn crc8(&self,cfg:GdpWireConfig)->u8{
        let mut c=Crc8::new();c.update_bits((self.version&0x03)as u64,2);c.update_bits(self.packet_type.to_wire()as u64,4);c.update_bits(self.size_class as u8 as u64,4);c.update_bit(cfg.form_bit(self.addresses.form()));
        match self.addresses{GdpAddresses::Global{destination,source}=>{c.update_bits(destination.0,64);c.update_bits(source.0,64);}GdpAddresses::Local{destination,source,..}=>{c.update_bits(destination as u64,16);c.update_bits(source as u64,16);}}
        c.finalize()
    }
    pub fn encode_into(&self,cfg:GdpWireConfig,out:&mut[u8])->Result<usize>{
        if self.version>3||self.packet_type.to_wire()>15{return Err(Error::InvalidField)}
        if matches!(self.addresses,GdpAddresses::Local{..})&&self.hop_limit>15{return Err(Error::InvalidField)}
        let need=self.header_len();if out.len()<need{return Err(Error::BufferFull)}
        let form_bit=cfg.form_bit(self.addresses.form())as u32;let crc=self.crc8(cfg)as u32;let hop=match self.addresses{GdpAddresses::Global{..}=>self.hop_limit as u32,GdpAddresses::Local{..}=>(self.hop_limit&0xf)as u32};
        let word1=((self.version as u32&3)<<30)|((self.packet_type.to_wire()as u32&0xf)<<26)|((self.size_class as u8 as u32&0xf)<<22)|(form_bit<<21)|(crc<<8)|hop;
        out[..4].copy_from_slice(&word1.to_be_bytes());
        match self.addresses{GdpAddresses::Global{destination,source}=>{out[4..12].copy_from_slice(&destination.0.to_be_bytes());out[12..20].copy_from_slice(&source.0.to_be_bytes());}GdpAddresses::Local{destination,source,..}=>{let w=((destination as u32)<<16)|source as u32;out[4..8].copy_from_slice(&w.to_be_bytes());}}
        Ok(need)
    }
    #[cfg(feature="alloc")]
    pub fn encode(&self,cfg:GdpWireConfig)->Result<Vec<u8>>{let mut out=alloc::vec![0u8;self.header_len()];self.encode_into(cfg,&mut out)?;Ok(out)}

    /// Decode GDP using the selected backend. Handwritten Rust remains the
    /// default. `p4-gdp` selects the x4c-generated parser for the standard
    /// local-form-bit convention. `p4-gdp-compare` executes both decoders and
    /// rejects any difference in result or error classification.
    pub fn decode(buf:&[u8],cfg:GdpWireConfig,local_prefix:u64)->Result<Self>{
        #[cfg(feature="p4-gdp")]
        if cfg.local_form_bit {
            #[cfg(feature="p4-gdp-compare")]
            {
                let p4=crate::p4_gdp::decode_header(buf,local_prefix);
                let handwritten=Self::decode_handwritten(buf,cfg,local_prefix);
                return match (p4,handwritten) {
                    (Ok(a),Ok(b)) if a==b=>Ok(a),
                    (Err(a),Err(b)) if a==b=>Err(a),
                    _=>Err(Error::InvalidField),
                };
            }
            #[cfg(not(feature="p4-gdp-compare"))]
            {
                return crate::p4_gdp::decode_header(buf,local_prefix);
            }
        }
        Self::decode_handwritten(buf,cfg,local_prefix)
    }

    #[cfg(feature="p4-gdp")]
    #[doc(hidden)]
    pub fn decode_handwritten_reference(buf:&[u8],cfg:GdpWireConfig,local_prefix:u64)->Result<Self>{
        Self::decode_handwritten(buf,cfg,local_prefix)
    }

    fn decode_handwritten(buf:&[u8],cfg:GdpWireConfig,local_prefix:u64)->Result<Self>{
        if buf.len()<4{return Err(Error::InvalidLength)}let w=u32::from_be_bytes(buf[0..4].try_into().unwrap());let version=((w>>30)&3)as u8;let packet_type=GdpType::from_wire(((w>>26)&0xf)as u8);let size_class=SizeClass::from_wire(((w>>22)&0xf)as u8)?;let form=cfg.form_from_bit(((w>>21)&1)!=0);let received_crc=((w>>8)&0xff)as u8;
        let header=match form{AddressForm::Global=>{if buf.len()<20{return Err(Error::InvalidLength)}let hop=(w&0xff)as u8;let destination=GdpAddress(u64::from_be_bytes(buf[4..12].try_into().unwrap()));let source=GdpAddress(u64::from_be_bytes(buf[12..20].try_into().unwrap()));Self{version,packet_type,size_class,hop_limit:hop,addresses:GdpAddresses::Global{destination,source}}}AddressForm::Local=>{if buf.len()<8{return Err(Error::InvalidLength)}let hop=(w&0xf)as u8;let ids=u32::from_be_bytes(buf[4..8].try_into().unwrap());Self{version,packet_type,size_class,hop_limit:hop,addresses:GdpAddresses::Local{destination:(ids>>16)as u16,source:ids as u16,prefix:local_prefix&!0xffff}}}};
        if header.crc8(cfg)!=received_crc{return Err(Error::InvalidCrc)}Ok(header)
    }
}

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct GdpPacketRef<'a>{pub header:GdpHeader,pub payload:&'a[u8]}
impl<'a> GdpPacketRef<'a>{
    pub fn new(header:GdpHeader,payload:&'a[u8])->Result<Self>{if payload.len()!=header.size_class.bytes(){return Err(Error::InvalidLength)}Ok(Self{header,payload})}
    pub fn decode(buf:&'a[u8],cfg:GdpWireConfig,local_prefix:u64)->Result<Self>{let header=GdpHeader::decode(buf,cfg,local_prefix)?;let h=header.header_len();let total=h+header.size_class.bytes();if buf.len()!=total{return Err(Error::InvalidLength)}Ok(Self{header,payload:&buf[h..]})}
    pub fn encode_into(&self,cfg:GdpWireConfig,out:&mut[u8])->Result<usize>{let total=self.header.header_len()+self.payload.len();if out.len()<total{return Err(Error::BufferFull)}let h=self.header.encode_into(cfg,out)?;out[h..total].copy_from_slice(self.payload);Ok(total)}
}

#[cfg(feature="alloc")]
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct GdpPacket{pub header:GdpHeader,pub payload:Vec<u8>}
#[cfg(feature="alloc")]
impl GdpPacket{
    pub fn new(header:GdpHeader,payload:Vec<u8>)->Result<Self>{if payload.len()!=header.size_class.bytes(){return Err(Error::InvalidLength)}Ok(Self{header,payload})}
    pub fn encode(&self,cfg:GdpWireConfig)->Result<Vec<u8>>{let mut out=alloc::vec![0u8;self.header.header_len()+self.payload.len()];let n=GdpPacketRef::new(self.header.clone(),&self.payload)?.encode_into(cfg,&mut out)?;out.truncate(n);Ok(out)}
    pub fn decode(buf:&[u8],cfg:GdpWireConfig,local_prefix:u64)->Result<Self>{let borrowed=GdpPacketRef::decode(buf,cfg,local_prefix)?;Ok(Self{header:borrowed.header,payload:borrowed.payload.to_vec()})}
}
