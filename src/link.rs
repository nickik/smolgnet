use alloc::collections::VecDeque;
use alloc::vec::Vec;
use crate::error::{Error,Result};
use crate::wire::gdp::{AddressForm,GdpPacket,GdpWireConfig,SizeClass};

#[derive(Debug,Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash)]
pub struct Vcid(u8);
impl Vcid { pub const CONTROL:Self=Self(0); pub const VC1:Self=Self(1); pub const VC2:Self=Self(2); pub const VC3:Self=Self(3); pub fn new(v:u8)->Result<Self>{if v<4{Ok(Self(v))}else{Err(Error::InvalidField)}} pub const fn get(self)->u8{self.0} }

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct Flit { pub vcid:Vcid, pub data:u32 }

#[derive(Debug,Clone)]
struct RxSegment { bytes:Vec<u8>, expected_bytes:usize, expected_flits:usize, received_flits:usize }

#[derive(Debug,Clone)]
pub struct DlpEndpoint {
    cfg:GdpWireConfig,
    local_prefix:u64,
    tx_credit:u32,
    tx_queue:VecDeque<Flit>,
    rx:[Option<RxSegment>;4],
    desynchronized:[bool;4],
    link_up:bool,
}
impl DlpEndpoint {
    pub fn new(cfg:GdpWireConfig,local_prefix:u64)->Self{Self{cfg,local_prefix,tx_credit:0,tx_queue:VecDeque::new(),rx:[None,None,None,None],desynchronized:[false;4],link_up:true}}
    pub fn set_link_up(&mut self,up:bool){self.link_up=up;if !up{self.tx_queue.clear();self.rx=[None,None,None,None];self.desynchronized=[false;4];}}
    pub fn grant_tx_credit(&mut self,count:u32){self.tx_credit=self.tx_credit.saturating_add(count)}
    pub const fn tx_credit(&self)->u32{self.tx_credit}
    pub fn queued_flits(&self)->usize{self.tx_queue.len()}
    pub fn queue_packet(&mut self,vcid:Vcid,packet:&GdpPacket)->Result<usize>{if !self.link_up{return Err(Error::LinkDown)};let bytes=packet.encode(self.cfg)?;let n=(bytes.len()+3)/4;for i in 0..n{let start=i*4;let mut b=[0u8;4];let end=(start+4).min(bytes.len());b[..end-start].copy_from_slice(&bytes[start..end]);self.tx_queue.push_back(Flit{vcid,data:u32::from_be_bytes(b)});}Ok(n)}
    pub fn poll_tx(&mut self)->Result<Option<Flit>>{if !self.link_up{return Err(Error::LinkDown)};if self.tx_queue.is_empty(){return Ok(None)};if self.tx_credit==0{return Err(Error::NoCredit)};self.tx_credit-=1;Ok(self.tx_queue.pop_front())}
    pub fn receive(&mut self,flit:Flit)->Result<Option<GdpPacket>>{if !self.link_up{return Err(Error::LinkDown)};let idx=flit.vcid.get() as usize;if self.desynchronized[idx]{return Err(Error::Desynchronized)};if self.rx[idx].is_none(){let w=flit.data;let size=SizeClass::from_wire(((w>>22)&0xf) as u8)?;let form=self.cfg.form_from_bit(((w>>21)&1)!=0);let header_len=match form{AddressForm::Global=>20,AddressForm::Local=>8};let total=header_len+size.bytes();self.rx[idx]=Some(RxSegment{bytes:Vec::with_capacity(((total+3)/4)*4),expected_bytes:total,expected_flits:(total+3)/4,received_flits:0});}
        let s=self.rx[idx].as_mut().unwrap();s.bytes.extend_from_slice(&flit.data.to_be_bytes());s.received_flits+=1;if s.received_flits<s.expected_flits{return Ok(None)};let mut done=self.rx[idx].take().unwrap();done.bytes.truncate(done.expected_bytes);match GdpPacket::decode(&done.bytes,self.cfg,self.local_prefix){Ok(p)=>Ok(Some(p)),Err(e)=>{self.desynchronized[idx]=true;Err(e)}}}
    pub fn reset_vc(&mut self,vcid:Vcid){let i=vcid.get() as usize;self.rx[i]=None;self.desynchronized[i]=false;}
}
