use smolgnet::*;

#[test]
fn dlp_flit_roundtrip_and_credit() {
    let cfg=GdpWireConfig::default();let mut a=DlpEndpoint::new(cfg,0);let mut b=DlpEndpoint::new(cfg,0);
    let h=GdpHeader::global(GdpType::Gctl,SizeClass::Ctrl32,64,GdpAddress(1),GdpAddress(2));let p=GdpPacket::new(h,vec![0xaa;32]).unwrap();let n=a.queue_packet(Vcid::VC1,&p).unwrap();
    assert_eq!(a.poll_tx().unwrap_err(),Error::NoCredit);a.grant_tx_credit(n as u32);let mut got=None;for _ in 0..n{let f=a.poll_tx().unwrap().unwrap();got=b.receive(f).unwrap().or(got);}assert_eq!(got.unwrap(),p);assert_eq!(a.tx_credit(),0);
}

#[test]
fn dlp_handles_tiny_partial_last_flit() {
    let cfg=GdpWireConfig::default();let mut a=DlpEndpoint::new(cfg,0);let mut b=DlpEndpoint::new(cfg,0);let h=GdpHeader::global(GdpType::Gctl,SizeClass::Tiny3,64,GdpAddress(1),GdpAddress(2));let p=GdpPacket::new(h,vec![1,2,3]).unwrap();let n=a.queue_packet(Vcid::VC2,&p).unwrap();a.grant_tx_credit(n as u32);let mut got=None;for _ in 0..n{got=b.receive(a.poll_tx().unwrap().unwrap()).unwrap().or(got);}assert_eq!(got.unwrap(),p);
}

#[test]
fn invalid_header_desynchronizes_vc_until_reset() {
    let cfg=GdpWireConfig::default();let mut b=DlpEndpoint::new(cfg,0);let h=GdpHeader::global(GdpType::Gctl,SizeClass::Ctrl32,64,GdpAddress(1),GdpAddress(2));let p=GdpPacket::new(h,vec![0;32]).unwrap();let bytes=p.encode(cfg).unwrap();let mut words=bytes.chunks(4).map(|c|{let mut x=[0u8;4];x[..c.len()].copy_from_slice(c);u32::from_be_bytes(x)}).collect::<Vec<_>>();words[1]^=1;let vc=Vcid::VC1;let mut failed=false;for w in words{match b.receive(Flit{vcid:vc,data:w}){Err(Error::InvalidCrc)=>{failed=true;break},Err(e)=>panic!("unexpected {e:?}"),_=>{}}}assert!(failed);assert_eq!(b.receive(Flit{vcid:vc,data:0}).unwrap_err(),Error::Desynchronized);b.reset_vc(vc);
}
