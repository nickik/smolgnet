use smolgnet::*;
use smolgnet::wire::gts::Direction;

fn pair()->(Endpoint,Endpoint,DirectLink){(Endpoint::new(GdpAddress(0x0102_0304_0000_0001)),Endpoint::new(GdpAddress(0x0102_0304_0000_0002)),DirectLink::new(1_000_000))}

#[test]
fn gctl_echo_roundtrip() {
    let (mut a,mut b,mut link)=pair();a.send_echo(b.address(),0x1234,b"ping",SizeClass::Ctrl32).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let body=a.take_echo_reply(0x1234).unwrap();assert_eq!(&body[..4],b"ping");
}

#[test]
fn connect_and_reliable_message_roundtrip() {
    let (mut client,mut server,mut link)=pair();let file=ServiceSelector::registered(1).unwrap();server.listen(file,ListenerConfig::default());let profile=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ch=client.connect(server.address(),file,profile).unwrap();link.pump(&mut client,&mut server,0,10000).unwrap();let sh=server.accept().unwrap();assert_eq!(client.tunnel_state(ch).unwrap(),smolgnet::TunnelState::Established);client.send(ch,0,b"hello",10).unwrap();link.pump(&mut client,&mut server,10,10000).unwrap();assert_eq!(server.recv(sh,0).unwrap(),Some(b"hello".to_vec()));server.send(sh,0,b"world",20).unwrap();link.pump(&mut client,&mut server,20,10000).unwrap();assert_eq!(client.recv(ch,0).unwrap(),Some(b"world".to_vec()));
}

#[test]
fn secondary_unreliable_stream_and_stream_reset() {
    let (mut a,mut b,mut link)=pair();let file=ServiceSelector::registered(1).unwrap();b.listen(file,ListenerConfig::default());let base=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),file,base).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();let p=StreamProfile::unreliable_variable(SizeClass::Ctrl64,Direction::Bidirectional,true,false);let sid=a.open_stream(ah,p).unwrap();link.pump(&mut a,&mut b,1,10000).unwrap();a.send(ah,sid,b"voice",2).unwrap();link.pump(&mut a,&mut b,2,10000).unwrap();assert_eq!(b.recv(bh,sid).unwrap(),Some(b"voice".to_vec()));a.reset_stream(ah,sid,2).unwrap();link.pump(&mut a,&mut b,3,10000).unwrap();assert_eq!(a.stream_state(ah,sid).unwrap(),smolgnet::gts::StreamState::Reset);assert_eq!(b.stream_state(bh,sid).unwrap(),smolgnet::gts::StreamState::Reset);a.send(ah,0,b"tunnel lives",4).unwrap();link.pump(&mut a,&mut b,4,10000).unwrap();assert_eq!(b.recv(bh,0).unwrap(),Some(b"tunnel lives".to_vec()));
}

#[test]
fn stream_and_tunnel_graceful_close() {
    let (mut a,mut b,mut link)=pair();let file=ServiceSelector::registered(1).unwrap();b.listen(file,ListenerConfig::default());let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),file,p).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();a.close_stream(ah,0).unwrap();link.pump(&mut a,&mut b,1,10000).unwrap();assert_eq!(a.stream_state(ah,0).unwrap(),smolgnet::gts::StreamState::Closed);assert_eq!(b.stream_state(bh,0).unwrap(),smolgnet::gts::StreamState::Closed);a.close_tunnel(ah).unwrap();link.pump(&mut a,&mut b,2,10000).unwrap();assert_eq!(a.tunnel_state(ah).unwrap(),smolgnet::TunnelState::Closed);assert_eq!(b.tunnel_state(bh).unwrap(),smolgnet::TunnelState::Closed);
}

#[test]
fn tunnel_reset_is_immediate() {
    let (mut a,mut b,mut link)=pair();let file=ServiceSelector::registered(1).unwrap();b.listen(file,ListenerConfig::default());let p=StreamProfile::reliable_variable(SizeClass::Msg128,Direction::Bidirectional);let ah=a.connect(b.address(),file,p).unwrap();link.pump(&mut a,&mut b,0,10000).unwrap();let bh=b.accept().unwrap();a.reset_tunnel(ah,2).unwrap();link.pump(&mut a,&mut b,1,10000).unwrap();assert_eq!(a.tunnel_state(ah).unwrap(),smolgnet::TunnelState::Reset);assert_eq!(b.tunnel_state(bh).unwrap(),smolgnet::TunnelState::Reset);
}
