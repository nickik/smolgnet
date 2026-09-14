use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use crate::error::{Error,Result};
use crate::gts::{GtsTunnel, StreamState, TunnelState};
use crate::link::{DlpEndpoint,Flit,Vcid};
use crate::wire::css::ServiceSelector;
use crate::wire::gctl::{GctlMessage,GctlType};
use crate::wire::gdp::{GdpAddress,GdpHeader,GdpPacket,GdpType,GdpWireConfig,SizeClass};
use crate::wire::gts::{GtsContext,GtsPacket,GtsType,StreamProfile};

#[derive(Debug,Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash)]
pub struct TunnelHandle(pub u32);

#[derive(Debug,Clone,Copy)]
pub struct ListenerConfig { pub receive_slots:u8 }
impl Default for ListenerConfig { fn default()->Self{Self{receive_slots:32}} }

#[derive(Debug,Clone)]
struct TunnelRecord { peer:GdpAddress, tunnel:GtsTunnel }

#[derive(Debug,Clone)]
pub struct Endpoint {
    address:GdpAddress,
    dlp:DlpEndpoint,
    listeners:BTreeMap<ServiceSelector,ListenerConfig>,
    tunnels:BTreeMap<u32,TunnelRecord>,
    accepted:VecDeque<TunnelHandle>,
    next_tunnel:u32,
    next_reset:u32,
    echo_replies:BTreeMap<u32,Vec<u8>>,
    gctl_inbox:VecDeque<(GdpAddress,GctlMessage)>,
}
impl Endpoint {
    pub fn new(address:GdpAddress)->Self{let cfg=GdpWireConfig::default();Self{address,dlp:DlpEndpoint::new(cfg,0),listeners:BTreeMap::new(),tunnels:BTreeMap::new(),accepted:VecDeque::new(),next_tunnel:1,next_reset:0x1000_0001,echo_replies:BTreeMap::new(),gctl_inbox:VecDeque::new()}}
    pub const fn address(&self)->GdpAddress{self.address}
    pub fn dlp(&self)->&DlpEndpoint{&self.dlp}
    pub fn dlp_mut(&mut self)->&mut DlpEndpoint{&mut self.dlp}
    fn alloc_tunnel(&mut self)->(u32,u32){let t=self.next_tunnel;self.next_tunnel=self.next_tunnel.wrapping_add(1).max(1);let r=self.next_reset;self.next_reset=self.next_reset.wrapping_add(1).max(1);(t,r)}
    pub fn listen(&mut self,css:ServiceSelector,config:ListenerConfig){self.listeners.insert(css,config);}
    pub fn accept(&mut self)->Option<TunnelHandle>{self.accepted.pop_front()}
    pub fn tunnel_state(&self,h:TunnelHandle)->Result<TunnelState>{Ok(self.tunnels.get(&h.0).ok_or(Error::UnknownTunnel)?.tunnel.state)}
    pub fn stream_state(&self,h:TunnelHandle,id:u8)->Result<StreamState>{Ok(self.tunnels.get(&h.0).ok_or(Error::UnknownTunnel)?.tunnel.streams.get(&id).ok_or(Error::UnknownStream)?.state)}

    fn local_credit(profile:StreamProfile,local_is_opener:bool,slots:u8)->u8{if profile.unreliable{return 0} let receives=if local_is_opener{profile.direction.peer_may_send()}else{profile.direction.opener_may_send()};if receives{slots}else{0}}

    pub fn connect(&mut self,remote:GdpAddress,css:ServiceSelector,profile:StreamProfile)->Result<TunnelHandle>{profile.validate()?;let (local,reset)=self.alloc_tunnel();let credit=Self::local_credit(profile,true,32);let tunnel=GtsTunnel::initiator(local,reset,css,profile,credit)?;let packet=GtsPacket::Connect{initiator_receive_tunnel:local,initiator_reset_id:reset,profile,initial_receive_credit:credit,css};self.tunnels.insert(local,TunnelRecord{peer:remote,tunnel});self.send_gts(remote,packet,None)?;Ok(TunnelHandle(local))}

    pub fn open_stream(&mut self,h:TunnelHandle,profile:StreamProfile)->Result<u8>{profile.validate()?;let (peer,id,credit,remote_id)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;let id=rec.tunnel.alloc_stream_id()?;let credit=Self::local_credit(profile,true,32);rec.tunnel.add_stream(id,profile,true,credit,0)?;rec.tunnel.stream_mut(id)?.mark_opening();(rec.peer,id,credit,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::StreamOpen{tunnel_id:remote_id,stream_id:id,profile,initial_receive_credit:credit},None)?;Ok(id)}

    pub fn send(&mut self,h:TunnelHandle,stream_id:u8,data:&[u8],now:u64)->Result<()>{self.send_inner(h,stream_id,data,false,now)}
    pub fn send_end(&mut self,h:TunnelHandle,stream_id:u8,data:&[u8],now:u64)->Result<()>{self.send_inner(h,stream_id,data,true,now)}
    fn send_inner(&mut self,h:TunnelHandle,stream_id:u8,data:&[u8],end:bool,now:u64)->Result<()>{let (peer,profile,packet)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;if rec.tunnel.state!=TunnelState::Established{return Err(Error::InvalidState)};let remote=rec.tunnel.remote_id()?;let s=rec.tunnel.stream_mut(stream_id)?;let p=s.profile;let packet=s.send_packet(remote,data.to_vec(),end,now)?;(rec.peer,p,packet)};self.send_gts(peer,packet,Some(profile))}
    pub fn recv(&mut self,h:TunnelHandle,stream_id:u8)->Result<Option<Vec<u8>>>{Ok(self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?.tunnel.stream_mut(stream_id)?.recv())}

    pub fn reset_stream(&mut self,h:TunnelHandle,stream_id:u8,reason:u8)->Result<()>{let (peer,remote)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;rec.tunnel.stream_mut(stream_id)?.state=StreamState::Reset;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::StreamReset{tunnel_id:remote,stream_id,reason,ack:false},None)}
    pub fn close_stream(&mut self,h:TunnelHandle,stream_id:u8)->Result<()>{let (peer,remote)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;rec.tunnel.stream_mut(stream_id)?.state=StreamState::Closing;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::StreamClose{tunnel_id:remote,stream_id,final_sequence:0,ack:false},None)}
    pub fn close_tunnel(&mut self,h:TunnelHandle)->Result<()>{let (peer,remote)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;rec.tunnel.state=TunnelState::Closing;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::TunnelClose{tunnel_id:remote,ack:false},None)}
    pub fn reset_tunnel(&mut self,h:TunnelHandle,reason:u8)->Result<()>{let (peer,remote,reset)={let rec=self.tunnels.get_mut(&h.0).ok_or(Error::UnknownTunnel)?;rec.tunnel.state=TunnelState::Reset;(rec.peer,rec.tunnel.remote_id()?,rec.tunnel.remote_reset_id.ok_or(Error::InvalidState)?)};self.send_gts(peer,GtsPacket::Reset{tunnel_id:remote,reset_id:reset,reason},None)}

    pub fn send_echo(&mut self,remote:GdpAddress,transaction_id:u32,body:&[u8],class:SizeClass)->Result<()>{let m=GctlMessage::new(GctlType::EchoRequest,transaction_id,body.to_vec());self.send_gctl(remote,m,class)}
    pub fn take_echo_reply(&mut self,transaction_id:u32)->Option<Vec<u8>>{self.echo_replies.remove(&transaction_id)}
    pub fn take_gctl(&mut self)->Option<(GdpAddress,GctlMessage)>{self.gctl_inbox.pop_front()}
    pub fn send_gctl(&mut self,remote:GdpAddress,msg:GctlMessage,class:SizeClass)->Result<()>{let payload=msg.encode_exact(class.bytes())?;let h=GdpHeader::global(GdpType::Gctl,class,64,self.address,remote);self.dlp.queue_packet(Vcid::VC1,&GdpPacket::new(h,payload)?)?;Ok(())}
    fn send_gts(&mut self,remote:GdpAddress,packet:GtsPacket,profile:Option<StreamProfile>)->Result<()>{let class=packet.choose_size_class(profile)?;let ctx=GtsContext{gdp_version:0,size_class:class,source:self.address,destination:remote};let payload=packet.encode(ctx,profile)?;let h=GdpHeader::global(GdpType::Gts,class,64,self.address,remote);self.dlp.queue_packet(Vcid::VC1,&GdpPacket::new(h,payload)?)?;Ok(())}

    pub fn poll_tx_flit(&mut self)->Result<Option<Flit>>{self.dlp.poll_tx()}
    pub fn receive_flit(&mut self,flit:Flit,now:u64)->Result<bool>{if let Some(packet)=self.dlp.receive(flit)?{self.handle_gdp(packet,now)?;Ok(true)}else{Ok(false)}}
    fn handle_gdp(&mut self,packet:GdpPacket,now:u64)->Result<()>{if packet.header.destination()!=self.address{return Err(Error::InvalidField)};let src=packet.header.source();match packet.header.packet_type{GdpType::Gctl=>self.handle_gctl(src,packet),GdpType::Gts=>self.handle_gts(src,packet,now),GdpType::Reserved(_)=>Err(Error::Unsupported)}}
    fn handle_gctl(&mut self,src:GdpAddress,packet:GdpPacket)->Result<()>{let msg=GctlMessage::decode(&packet.payload)?;match msg.message_type{GctlType::EchoRequest=>{let reply=msg.echo_reply()?;self.send_gctl(src,reply,packet.header.size_class)},GctlType::EchoReply=>{self.echo_replies.insert(msg.transaction_id,msg.body);Ok(())},_=>{self.gctl_inbox.push_back((src,msg));Ok(())}}}

    fn gts_profile_for(&self,payload:&[u8])->Result<Option<StreamProfile>>{if payload.is_empty(){return Err(Error::InvalidLength)};let ty=GtsType::from_wire(payload[0]&0xf)?;if !matches!(ty,GtsType::Data|GtsType::DataEnd|GtsType::Datagram){return Ok(None)};if payload.len()<6{return Err(Error::InvalidLength)};let local=u32::from_be_bytes(payload[1..5].try_into().unwrap());let stream=payload[5];let rec=self.tunnels.get(&local).ok_or(Error::UnknownTunnel)?;Ok(Some(rec.tunnel.streams.get(&stream).ok_or(Error::UnknownStream)?.profile))}
    fn handle_gts(&mut self,src:GdpAddress,packet:GdpPacket,_now:u64)->Result<()>{let profile=self.gts_profile_for(&packet.payload)?;let ctx=GtsContext{gdp_version:packet.header.version,size_class:packet.header.size_class,source:src,destination:self.address};let g=GtsPacket::decode(&packet.payload,ctx,profile)?;match g{
        GtsPacket::Connect{initiator_receive_tunnel,initiator_reset_id,profile,initial_receive_credit,css}=>{let listener=*self.listeners.get(&css).ok_or(Error::UnknownService)?;let (local,reset)=self.alloc_tunnel();let local_credit=Self::local_credit(profile,false,listener.receive_slots);let tunnel=GtsTunnel::responder(local,reset,initiator_receive_tunnel,initiator_reset_id,css,profile,local_credit,initial_receive_credit)?;self.tunnels.insert(local,TunnelRecord{peer:src,tunnel});self.accepted.push_back(TunnelHandle(local));self.send_gts(src,GtsPacket::ConnectAck{initiator_receive_tunnel,responder_receive_tunnel:local,responder_reset_id:reset,status:0,initial_receive_credit:local_credit},None)}
        GtsPacket::ConnectAck{initiator_receive_tunnel,responder_receive_tunnel,responder_reset_id,status,initial_receive_credit}=>{if status!=0{return Err(Error::UnknownService)};let rec=self.tunnels.get_mut(&initiator_receive_tunnel).ok_or(Error::UnknownTunnel)?;rec.tunnel.establish_initiator(responder_receive_tunnel,responder_reset_id,initial_receive_credit)}
        GtsPacket::StreamOpen{tunnel_id,stream_id,profile,initial_receive_credit}=>{let (peer,remote,credit)={let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;let local_credit=Self::local_credit(profile,false,32);rec.tunnel.add_stream(stream_id,profile,false,local_credit,initial_receive_credit)?;(rec.peer,rec.tunnel.remote_id()?,local_credit)};self.send_gts(peer,GtsPacket::StreamAck{tunnel_id:remote,stream_id,status:0,initial_receive_credit:credit},None)}
        GtsPacket::StreamAck{tunnel_id,stream_id,status,initial_receive_credit}=>{if status!=0{return Err(Error::ProfileViolation)};self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?.tunnel.stream_mut(stream_id)?.apply_open_ack(initial_receive_credit)}
        GtsPacket::Data{tunnel_id,stream_id,sequence,data,end}=>{let (peer,remote,ack,profile)={let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;let s=rec.tunnel.stream_mut(stream_id)?;let profile=s.profile;let ack=s.receive_packet(&GtsPacket::Data{tunnel_id,stream_id,sequence,data,end})?;(rec.peer,rec.tunnel.remote_id()?,ack,profile)};if let Some(GtsPacket::Ack{stream_id,ack_base,receive_bitmap,receive_credit,..})=ack{self.send_gts(peer,GtsPacket::Ack{tunnel_id:remote,stream_id,ack_base,receive_bitmap,receive_credit},Some(profile))?;}Ok(())}
        GtsPacket::Datagram{tunnel_id,stream_id,sequence,data}=>{let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;rec.tunnel.stream_mut(stream_id)?.receive_packet(&GtsPacket::Datagram{tunnel_id,stream_id,sequence,data})?;Ok(())}
        GtsPacket::Ack{tunnel_id,stream_id,ack_base,receive_bitmap,receive_credit}=>{self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?.tunnel.stream_mut(stream_id)?.on_ack(ack_base,receive_bitmap,receive_credit)}
        GtsPacket::StreamClose{tunnel_id,stream_id,final_sequence,ack:false}=>{let (peer,remote)={let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;rec.tunnel.stream_mut(stream_id)?.state=StreamState::Closed;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::StreamClose{tunnel_id:remote,stream_id,final_sequence,ack:true},None)}
        GtsPacket::StreamClose{tunnel_id,stream_id,ack:true,..}=>{self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?.tunnel.stream_mut(stream_id)?.state=StreamState::Closed;Ok(())}
        GtsPacket::StreamReset{tunnel_id,stream_id,reason,ack:false}=>{let (peer,remote)={let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;rec.tunnel.stream_mut(stream_id)?.state=StreamState::Reset;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::StreamReset{tunnel_id:remote,stream_id,reason,ack:true},None)}
        GtsPacket::StreamReset{tunnel_id,stream_id,ack:true,..}=>{self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?.tunnel.stream_mut(stream_id)?.state=StreamState::Reset;Ok(())}
        GtsPacket::TunnelClose{tunnel_id,ack:false}=>{let (peer,remote)={let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;rec.tunnel.state=TunnelState::Closed;(rec.peer,rec.tunnel.remote_id()?)};self.send_gts(peer,GtsPacket::TunnelClose{tunnel_id:remote,ack:true},None)}
        GtsPacket::TunnelClose{tunnel_id,ack:true}=>{self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?.tunnel.state=TunnelState::Closed;Ok(())}
        GtsPacket::Reset{tunnel_id,reset_id,..}=>{let rec=self.tunnels.get_mut(&tunnel_id).ok_or(Error::UnknownTunnel)?;if reset_id!=rec.tunnel.local_reset_id{return Err(Error::InvalidField)};rec.tunnel.state=TunnelState::Reset;Ok(())}
    }}

    pub fn tick(&mut self,now:u64)->Result<usize>{let mut pending=Vec::new();for rec in self.tunnels.values_mut(){if rec.tunnel.state!=TunnelState::Established{continue}let remote=rec.tunnel.remote_id()?;for s in rec.tunnel.streams.values_mut(){let p=s.profile;for pkt in s.retransmit_due(remote,now){pending.push((rec.peer,p,pkt));}}}let n=pending.len();for (peer,p,pkt) in pending{self.send_gts(peer,pkt,Some(p))?;}Ok(n)}
}

#[derive(Debug,Clone)]
pub struct DirectLink { initial_credit:u32, attached:bool }
impl DirectLink {
    pub fn new(initial_credit:u32)->Self{Self{initial_credit,attached:false}}
    pub fn attach(&mut self,a:&mut Endpoint,b:&mut Endpoint){if !self.attached{a.dlp_mut().grant_tx_credit(self.initial_credit);b.dlp_mut().grant_tx_credit(self.initial_credit);self.attached=true;}}
    pub fn pump(&mut self,a:&mut Endpoint,b:&mut Endpoint,now:u64,max_flits:usize)->Result<usize>{self.attach(a,b);let mut moved=0;loop{if moved>=max_flits{return Err(Error::BufferFull)};let mut progress=false;match a.poll_tx_flit(){Ok(Some(f))=>{b.receive_flit(f,now)?;moved+=1;progress=true},Ok(None)|Err(Error::NoCredit)=>{},Err(e)=>return Err(e)};match b.poll_tx_flit(){Ok(Some(f))=>{a.receive_flit(f,now)?;moved+=1;progress=true},Ok(None)|Err(Error::NoCredit)=>{},Err(e)=>return Err(e)};if !progress{break}}Ok(moved)}
}
