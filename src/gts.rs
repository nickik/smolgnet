use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use crate::error::{Error, Result};
use crate::wire::css::ServiceSelector;
use crate::wire::gts::{GtsPacket, StreamProfile};

pub use crate::wire::gts::Direction;

pub const DEFAULT_RTO_MS: u64 = 500;
pub const DEFAULT_RX_SLOTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelRole { Initiator, Responder }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState { Connecting, Established, Closing, Closed, Reset }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState { Opening, Open, Closing, Closed, Reset }

#[derive(Debug, Clone)]
struct PendingTx {
    data: Vec<u8>,
    end: bool,
    sent_at: u64,
}

#[derive(Debug, Clone)]
pub struct ReliableTx {
    next_sequence: u32,
    peer_credit: u8,
    outstanding: BTreeMap<u32, PendingTx>,
    rto_ms: u64,
}
impl ReliableTx {
    pub fn new(peer_credit:u8)->Self{Self{next_sequence:0,peer_credit,outstanding:BTreeMap::new(),rto_ms:DEFAULT_RTO_MS}}
    pub fn peer_credit(&self)->u8{self.peer_credit}
    pub fn outstanding(&self)->usize{self.outstanding.len()}
    pub fn queue(&mut self,data:Vec<u8>,end:bool,now:u64)->Result<u32>{if self.peer_credit==0{return Err(Error::WouldBlock)};let seq=self.next_sequence;self.next_sequence=self.next_sequence.wrapping_add(1);self.peer_credit-=1;self.outstanding.insert(seq,PendingTx{data,end,sent_at:now});Ok(seq)}
    pub fn on_ack(&mut self,base:u32,bitmap:u32,credit:u8){
        let keys:Vec<u32>=self.outstanding.keys().copied().collect();
        for k in keys { let acked = if k<=base {true} else { let d=k.wrapping_sub(base).wrapping_sub(1); d<32 && (bitmap&(1u32<<d))!=0 }; if acked {self.outstanding.remove(&k);} }
        self.peer_credit=credit;
    }
    pub fn due(&mut self,now:u64)->Vec<(u32,Vec<u8>,bool)>{let mut out=Vec::new();for (&seq,p) in self.outstanding.iter_mut(){if now.saturating_sub(p.sent_at)>=self.rto_ms{p.sent_at=now;out.push((seq,p.data.clone(),p.end));}}out}
}

#[derive(Debug, Clone)]
pub struct ReliableRx {
    next_expected:u32,
    pending:BTreeMap<u32,Vec<u8>>,
    delivered:VecDeque<Vec<u8>>,
    capacity:usize,
    seen_any:bool,
}
impl ReliableRx {
    pub fn new(capacity:usize)->Self{Self{next_expected:0,pending:BTreeMap::new(),delivered:VecDeque::new(),capacity,seen_any:false}}
    fn used(&self)->usize{self.pending.len()+self.delivered.len()}
    pub fn credit(&self)->u8{self.capacity.saturating_sub(self.used()).min(255) as u8}
    pub fn receive(&mut self,seq:u32,data:Vec<u8>)->Result<bool>{
        if seq<self.next_expected{return Ok(false)}
        if seq==self.next_expected { if self.used()>=self.capacity{return Err(Error::BufferFull)};self.delivered.push_back(data);self.seen_any=true;self.next_expected=self.next_expected.wrapping_add(1);loop{let n=self.next_expected;if let Some(v)=self.pending.remove(&n){self.delivered.push_back(v);self.next_expected=self.next_expected.wrapping_add(1);}else{break}};return Ok(true)}
        let distance=seq.wrapping_sub(self.next_expected);if distance>32{return Err(Error::BufferFull)};if self.used()>=self.capacity{return Err(Error::BufferFull)};self.pending.entry(seq).or_insert(data);Ok(false)
    }
    pub fn ack(&self)->Option<(u32,u32,u8)>{if !self.seen_any{return None};let base=self.next_expected.wrapping_sub(1);let mut bitmap=0u32;for &seq in self.pending.keys(){let d=seq.wrapping_sub(base).wrapping_sub(1);if d<32{bitmap|=1u32<<d;}}Some((base,bitmap,self.credit()))}
    pub fn recv(&mut self)->Option<Vec<u8>>{self.delivered.pop_front()}
}

#[derive(Debug, Clone)]
pub struct GtsStream {
    pub id:u8,
    pub profile:StreamProfile,
    pub state:StreamState,
    pub opened_by_local:bool,
    reliable_tx:Option<ReliableTx>,
    reliable_rx:Option<ReliableRx>,
    unrel_tx_sequence:u32,
    unrel_rx_highest:Option<u32>,
    unrel_delivered:VecDeque<Vec<u8>>,
}
impl GtsStream {
    pub fn new(id:u8,profile:StreamProfile,opened_by_local:bool,local_receive_credit:u8,peer_receive_credit:u8)->Result<Self>{profile.validate()?;let reliable_tx=if profile.unreliable{None}else{Some(ReliableTx::new(peer_receive_credit))};let reliable_rx=if profile.unreliable{None}else{Some(ReliableRx::new(local_receive_credit.max(1) as usize))};Ok(Self{id,profile,state:StreamState::Open,opened_by_local,reliable_tx,reliable_rx,unrel_tx_sequence:0,unrel_rx_highest:None,unrel_delivered:VecDeque::new()})}
    fn local_may_send(&self,local_is_opener:bool)->bool{if local_is_opener{self.profile.direction.opener_may_send()}else{self.profile.direction.peer_may_send()}}
    fn remote_may_send(&self,local_is_opener:bool)->bool{if local_is_opener{self.profile.direction.peer_may_send()}else{self.profile.direction.opener_may_send()}}
    pub fn send_packet(&mut self,tunnel_id:u32,data:Vec<u8>,end:bool,now:u64)->Result<GtsPacket>{if self.state!=StreamState::Open{return Err(Error::InvalidState)};if !self.local_may_send(self.opened_by_local){return Err(Error::DirectionViolation)};if self.profile.unreliable{if end{return Err(Error::ProfileViolation)};let seq=if self.profile.sequenced{let s=self.unrel_tx_sequence;self.unrel_tx_sequence=self.unrel_tx_sequence.wrapping_add(1);Some(s)}else{None};Ok(GtsPacket::Datagram{tunnel_id,stream_id:self.id,sequence:seq,data})}else{let tx=self.reliable_tx.as_mut().unwrap();let seq=tx.queue(data.clone(),end,now)?;Ok(GtsPacket::Data{tunnel_id,stream_id:self.id,sequence:seq,data,end})}}
    pub fn receive_packet(&mut self,packet:&GtsPacket)->Result<Option<GtsPacket>>{if !self.remote_may_send(self.opened_by_local){return Err(Error::DirectionViolation)};match packet{
        GtsPacket::Data{tunnel_id,stream_id,sequence,data,..} if !self.profile.unreliable=>{if *stream_id!=self.id{return Err(Error::UnknownStream)};let rx=self.reliable_rx.as_mut().unwrap();rx.receive(*sequence,data.clone())?;if let Some((base,bm,credit))=rx.ack(){Ok(Some(GtsPacket::Ack{tunnel_id:*tunnel_id,stream_id:*stream_id,ack_base:base,receive_bitmap:bm,receive_credit:credit}))}else{Ok(None)}}
        GtsPacket::Datagram{stream_id,sequence,data,..} if self.profile.unreliable=>{if *stream_id!=self.id{return Err(Error::UnknownStream)};if self.profile.sequenced{let s=sequence.ok_or(Error::ProfileViolation)?;if let Some(h)=self.unrel_rx_highest{if s<=h{return Ok(None)}}self.unrel_rx_highest=Some(s);}self.unrel_delivered.push_back(data.clone());Ok(None)}
        _=>Err(Error::ProfileViolation)}}
    pub fn on_ack(&mut self,base:u32,bitmap:u32,credit:u8)->Result<()>{self.reliable_tx.as_mut().ok_or(Error::ProfileViolation)?.on_ack(base,bitmap,credit);Ok(())}
    pub fn recv(&mut self)->Option<Vec<u8>>{if self.profile.unreliable{self.unrel_delivered.pop_front()}else{self.reliable_rx.as_mut().unwrap().recv()}}
    pub fn retransmit_due(&mut self,tunnel_id:u32,now:u64)->Vec<GtsPacket>{let Some(tx)=self.reliable_tx.as_mut() else{return Vec::new()};tx.due(now).into_iter().map(|(sequence,data,end)|GtsPacket::Data{tunnel_id,stream_id:self.id,sequence,data,end}).collect()}
    pub fn peer_credit(&self)->Option<u8>{self.reliable_tx.as_ref().map(|x|x.peer_credit())}
    pub fn apply_open_ack(&mut self, peer_credit:u8)->Result<()> { if self.state != StreamState::Opening { return Err(Error::InvalidState); } if let Some(tx)=self.reliable_tx.as_mut(){ tx.peer_credit=peer_credit; } self.state=StreamState::Open; Ok(()) }
    pub fn mark_opening(&mut self){ self.state=StreamState::Opening; }
}

#[derive(Debug, Clone)]
pub struct GtsTunnel {
    pub role:TunnelRole,
    pub state:TunnelState,
    pub local_receive_id:u32,
    pub remote_receive_id:Option<u32>,
    pub local_reset_id:u32,
    pub remote_reset_id:Option<u32>,
    pub css:ServiceSelector,
    pub streams:BTreeMap<u8,GtsStream>,
    next_stream_id:u8,
}
impl GtsTunnel {
    pub fn initiator(local_receive_id:u32,local_reset_id:u32,css:ServiceSelector,stream0_profile:StreamProfile,local_credit:u8)->Result<Self>{let mut streams=BTreeMap::new();streams.insert(0,GtsStream::new(0,stream0_profile,true,local_credit,0)?);Ok(Self{role:TunnelRole::Initiator,state:TunnelState::Connecting,local_receive_id,remote_receive_id:None,local_reset_id,remote_reset_id:None,css,streams,next_stream_id:2})}
    pub fn responder(local_receive_id:u32,local_reset_id:u32,remote_receive_id:u32,remote_reset_id:u32,css:ServiceSelector,stream0_profile:StreamProfile,local_credit:u8,peer_credit:u8)->Result<Self>{let mut streams=BTreeMap::new();streams.insert(0,GtsStream::new(0,stream0_profile,false,local_credit,peer_credit)?);Ok(Self{role:TunnelRole::Responder,state:TunnelState::Established,local_receive_id,remote_receive_id:Some(remote_receive_id),local_reset_id,remote_reset_id:Some(remote_reset_id),css,streams,next_stream_id:1})}
    pub fn establish_initiator(&mut self,remote_receive_id:u32,remote_reset_id:u32,peer_credit:u8)->Result<()>{if self.state!=TunnelState::Connecting{return Err(Error::InvalidState)};self.remote_receive_id=Some(remote_receive_id);self.remote_reset_id=Some(remote_reset_id);self.state=TunnelState::Established;let s=self.streams.get_mut(&0).unwrap();if let Some(tx)=s.reliable_tx.as_mut(){tx.peer_credit=peer_credit;}Ok(())}
    pub fn alloc_stream_id(&mut self)->Result<u8>{if self.state!=TunnelState::Established{return Err(Error::InvalidState)};let id=self.next_stream_id;if id>253{return Err(Error::BufferFull)};self.next_stream_id=self.next_stream_id.wrapping_add(2);Ok(id)}
    pub fn add_stream(&mut self,id:u8,profile:StreamProfile,opened_by_local:bool,local_credit:u8,peer_credit:u8)->Result<()>{if self.streams.contains_key(&id){return Err(Error::InvalidState)};self.streams.insert(id,GtsStream::new(id,profile,opened_by_local,local_credit,peer_credit)?);Ok(())}
    pub fn stream_mut(&mut self,id:u8)->Result<&mut GtsStream>{self.streams.get_mut(&id).ok_or(Error::UnknownStream)}
    pub fn remote_id(&self)->Result<u32>{self.remote_receive_id.ok_or(Error::InvalidState)}
}
