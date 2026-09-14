use alloc::vec;
use alloc::vec::Vec;
use crate::error::{Error,Result};

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
#[repr(u8)]
pub enum GctlType {
    Solicit=0x01, Advertise=0x02, CreditRequest=0x03, Credit=0x04,
    AddressOffer=0x10, AddressClaim=0x11, AddressAck=0x12, AddressNak=0x13,
    EchoRequest=0x20, EchoReply=0x21, DestinationUnreachable=0x22, HopLimitExceeded=0x23,
    ParameterProblem=0x24, ClassUnsupported=0x25, TransitAborted=0x26, PathProbe=0x28, PathReply=0x29,
    StatusRequest=0x30, StatusReply=0x31, GetRequest=0x32, GetReply=0x33, GetNextRequest=0x34, GetNextReply=0x35, EventReport=0x36,
    Experimental=0xff,
}
impl GctlType { pub fn from_wire(v:u8)->Result<Self>{ use GctlType::*; Ok(match v {0x01=>Solicit,0x02=>Advertise,0x03=>CreditRequest,0x04=>Credit,0x10=>AddressOffer,0x11=>AddressClaim,0x12=>AddressAck,0x13=>AddressNak,0x20=>EchoRequest,0x21=>EchoReply,0x22=>DestinationUnreachable,0x23=>HopLimitExceeded,0x24=>ParameterProblem,0x25=>ClassUnsupported,0x26=>TransitAborted,0x28=>PathProbe,0x29=>PathReply,0x30=>StatusRequest,0x31=>StatusReply,0x32=>GetRequest,0x33=>GetReply,0x34=>GetNextRequest,0x35=>GetNextReply,0x36=>EventReport,0xff=>Experimental,_=>return Err(Error::Unsupported)}) } }

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct GctlMessage { pub version:u8,pub message_type:GctlType,pub code:u8,pub flags:u8,pub transaction_id:u32,pub body:Vec<u8> }
impl GctlMessage {
    pub fn new(message_type:GctlType,transaction_id:u32,body:Vec<u8>)->Self{Self{version:1,message_type,code:0,flags:0,transaction_id,body}}
    pub fn encoded_len(&self)->usize{8+self.body.len()}
    pub fn encode_exact(&self,total:usize)->Result<Vec<u8>>{if total<self.encoded_len(){return Err(Error::InvalidLength)};let mut o=vec![0u8;total];o[0]=self.version;o[1]=self.message_type as u8;o[2]=self.code;o[3]=self.flags;o[4..8].copy_from_slice(&self.transaction_id.to_be_bytes());o[8..8+self.body.len()].copy_from_slice(&self.body);Ok(o)}
    pub fn decode(buf:&[u8])->Result<Self>{if buf.len()<8{return Err(Error::InvalidLength)};Ok(Self{version:buf[0],message_type:GctlType::from_wire(buf[1])?,code:buf[2],flags:buf[3],transaction_id:u32::from_be_bytes(buf[4..8].try_into().unwrap()),body:buf[8..].to_vec()})}
    pub fn echo_reply(&self)->Result<Self>{if self.message_type!=GctlType::EchoRequest{return Err(Error::InvalidState)};Ok(Self{version:self.version,message_type:GctlType::EchoReply,code:self.code,flags:self.flags,transaction_id:self.transaction_id,body:self.body.clone()})}
}
