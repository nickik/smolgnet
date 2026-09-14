#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceSelector(pub [u8;16]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssWire {
    Registered(u8),
    Short([u8;4]),
    Full([u8;16]),
}

fn valid_short(bytes:&[u8;4])->bool { bytes.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) }
fn registered_to_short(code:u8)->Option<[u8;4]>{ match code {1=>Some(*b"FILE"),2=>Some(*b"GRPC"),_=>None} }
fn short_to_registered(v:&[u8;4])->Option<u8>{ if v==b"FILE"{Some(1)}else if v==b"GRPC"{Some(2)}else{None} }

impl ServiceSelector {
    pub const ZERO:Self=Self([0;16]);
    pub fn registered(code:u8)->Result<Self>{ let s=registered_to_short(code).ok_or(Error::InvalidField)?; Self::short_raw(s) }
    pub fn short(v:[u8;4])->Result<Self>{ if short_to_registered(&v).is_some(){return Err(Error::NonCanonical)}; Self::short_raw(v) }
    fn short_raw(v:[u8;4])->Result<Self>{ if !valid_short(&v){return Err(Error::InvalidField)}; let mut x=[0u8;16]; x[..4].copy_from_slice(&v); Ok(Self(x)) }
    pub fn full(v:[u8;16])->Result<Self>{ if v==[0;16]{return Err(Error::InvalidField)}; if v[4..].iter().all(|&b|b==0) { let a:[u8;4]=v[..4].try_into().unwrap(); if valid_short(&a){return Err(Error::NonCanonical)} } Ok(Self(v)) }
    pub fn canonical_wire(&self)->Result<CssWire>{ if self.0==[0;16]{return Err(Error::InvalidField)}; if self.0[4..].iter().all(|&b|b==0){ let s:[u8;4]=self.0[..4].try_into().unwrap(); if valid_short(&s){ if let Some(c)=short_to_registered(&s){return Ok(CssWire::Registered(c))} return Ok(CssWire::Short(s)); } } Ok(CssWire::Full(self.0)) }

    pub fn encoded_len(&self) -> Result<usize> {
        Ok(match self.canonical_wire()? { CssWire::Registered(_) => 2, CssWire::Short(_) => 5, CssWire::Full(_) => 17 })
    }

    pub fn encode_into(&self, out: &mut [u8]) -> Result<usize> {
        let wire = self.canonical_wire()?;
        let need = match wire { CssWire::Registered(_) => 2, CssWire::Short(_) => 5, CssWire::Full(_) => 17 };
        if out.len() < need { return Err(Error::BufferFull); }
        match wire {
            CssWire::Registered(c) => { out[0]=0; out[1]=c; }
            CssWire::Short(s) => { out[0]=0x40; out[1..5].copy_from_slice(&s); }
            CssWire::Full(v) => { out[0]=0x80; out[1..17].copy_from_slice(&v); }
        }
        Ok(need)
    }

    #[cfg(feature = "alloc")]
    pub fn encode(&self)->Result<Vec<u8>>{
        let mut out = Vec::with_capacity(self.encoded_len()?);
        out.resize(self.encoded_len()?, 0);
        let n = self.encode_into(&mut out)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn decode(buf:&[u8])->Result<(Self,usize)>{ if buf.is_empty(){return Err(Error::InvalidLength)}; if buf[0]&0x3f!=0{return Err(Error::InvalidField)}; match buf[0]>>6 { 0=>{if buf.len()<2{return Err(Error::InvalidLength)}; Ok((Self::registered(buf[1])?,2))}, 1=>{if buf.len()<5{return Err(Error::InvalidLength)}; let s:[u8;4]=buf[1..5].try_into().unwrap(); if short_to_registered(&s).is_some(){return Err(Error::NonCanonical)}; Ok((Self::short(s)?,5))}, 2=>{if buf.len()<17{return Err(Error::InvalidLength)}; let v:[u8;16]=buf[1..17].try_into().unwrap(); Ok((Self::full(v)?,17))}, _=>Err(Error::InvalidField) } }
}
