use smolgnet::*;
use smolgnet::gts::{GtsStream, StreamState};
use smolgnet::wire::gts::{Direction, StreamProfile};

fn pair()->(Endpoint,Endpoint,DirectLink){(Endpoint::new(GdpAddress(0x1111_0000_0000_0001)),Endpoint::new(GdpAddress(0x1111_0000_0000_0002)),DirectLink::new(1_000_000))}

#[test]
fn stream_profile_roundtrip_and_invalid_reliable_flags() {
    let p=StreamProfile::unreliable_variable(SizeClass::Msg128,Direction::PeerToOpener,true,true);
    assert_eq!(StreamProfile::from_wire(p.to_wire().unwrap()).unwrap(),p);
    let bad=StreamProfile{unreliable:false,variable:true,sequenced:true,unchecked_payload:false,direction:Direction::Bidirectional,size_class:SizeClass::Msg128};
    assert_eq!(bad.validate().unwrap_err(),Error::ProfileViolation);
}

#[test]
fn unidirectional_stream_enforces_sender_direction() {
    let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::OpenerToPeer);
    let mut opener=GtsStream::new(0,p,true,0,1).unwrap();
    assert!(opener.send_packet(7,b"ok".to_vec(),false,0).is_ok());
    let mut peer=GtsStream::new(0,p,false,1,0).unwrap();
    assert_eq!(peer.send_packet(7,b"no".to_vec(),false,0).unwrap_err(),Error::DirectionViolation);
}

#[test]
fn sequenced_unreliable_drops_stale_messages() {
    let p=StreamProfile::unreliable_variable(SizeClass::Ctrl64,Direction::Bidirectional,true,false);
    let mut s=GtsStream::new(1,p,false,0,0).unwrap();
    s.receive_packet(&GtsPacket::Datagram{tunnel_id:1,stream_id:1,sequence:Some(1),data:b"new".to_vec()}).unwrap();
    s.receive_packet(&GtsPacket::Datagram{tunnel_id:1,stream_id:1,sequence:Some(0),data:b"old".to_vec()}).unwrap();
    assert_eq!(s.recv(),Some(b"new".to_vec()));
    assert_eq!(s.recv(),None);
}

#[test]
fn secondary_reliable_stream_roundtrip() {
    let (mut a,mut b,mut link)=pair();let css=ServiceSelector::registered(1).unwrap();b.listen(css,ListenerConfig::default());let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),css,p).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();let sid=a.open_stream(ah,p).unwrap();link.pump(&mut a,&mut b,1,10000).unwrap();assert_eq!(a.stream_state(ah,sid).unwrap(),StreamState::Open);a.send(ah,sid,b"second stream",2).unwrap();link.pump(&mut a,&mut b,2,10000).unwrap();assert_eq!(b.recv(bh,sid).unwrap(),Some(b"second stream".to_vec()));
}

#[test]
fn data_end_is_delivered_as_final_message_unit() {
    let (mut a,mut b,mut link)=pair();let css=ServiceSelector::registered(1).unwrap();b.listen(css,ListenerConfig::default());let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),css,p).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();a.send_end(ah,0,b"final",10).unwrap();link.pump(&mut a,&mut b,10,10000).unwrap();assert_eq!(b.recv(bh,0).unwrap(),Some(b"final".to_vec()));
}

#[test]
fn endpoint_tick_generates_retransmission() {
    let (mut a,mut b,mut link)=pair();let css=ServiceSelector::registered(1).unwrap();b.listen(css,ListenerConfig::default());let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),css,p).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();a.send(ah,0,b"retry",10).unwrap();assert_eq!(a.tick(509).unwrap(),0);assert_eq!(a.tick(510).unwrap(),1);link.pump(&mut a,&mut b,510,10000).unwrap();assert_eq!(b.recv(bh,0).unwrap(),Some(b"retry".to_vec()));assert_eq!(b.recv(bh,0).unwrap(),None);
}

#[test]
fn gctl_common_header_roundtrip() {
    let m=GctlMessage{version:1,message_type:GctlType::StatusRequest,code:3,flags:1,transaction_id:0x12345678,body:b"body".to_vec()};let bytes=m.encode_exact(32).unwrap();let d=GctlMessage::decode(&bytes).unwrap();assert_eq!(d.version,1);assert_eq!(d.message_type,GctlType::StatusRequest);assert_eq!(d.code,3);assert_eq!(d.flags,1);assert_eq!(d.transaction_id,0x12345678);assert_eq!(&d.body[..4],b"body");
}
