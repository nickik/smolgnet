use smolgnet::{
    BoundedAckState, BoundedGtsSocket, BoundedStreamSlot, BoundedTunnelRole, MessageSlot,
    RxMeta, ServiceSelector, SocketSet, SocketStorage, StreamProfile, TxMeta,
};
use smolgnet::wire::gdp::SizeClass;
use smolgnet::wire::gts::Direction;

#[test]
fn caller_supplied_socket_memory_runs_reliable_gts_without_alloc_api() {
    let mut stream_slots = [BoundedStreamSlot::EMPTY; 4];
    let mut tx_slots = [MessageSlot::<TxMeta>::EMPTY; 4];
    let mut tx_bytes = [0u8; 512];
    let mut rx_slots = [MessageSlot::<RxMeta>::EMPTY; 4];
    let mut rx_bytes = [0u8; 512];

    let mut tunnel = BoundedGtsSocket::new(
        BoundedTunnelRole::Initiator,
        100,
        200,
        ServiceSelector([0x41; 16]),
        &mut stream_slots,
        &mut tx_slots,
        &mut tx_bytes,
        &mut rx_slots,
        &mut rx_bytes,
    ).unwrap();
    tunnel.establish(101, 201).unwrap();

    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    tunnel.add_stream(0, profile, true, 4, 4).unwrap();

    let mut sockets = [SocketStorage::EMPTY];
    let mut set = SocketSet::new(&mut sockets);
    let handle = set.add(tunnel).unwrap();

    let sock = set.get_mut(handle).unwrap();
    assert_eq!(sock.send(0, b"heapless", false, 0).unwrap().sequence, Some(0));
    let retry = sock.retransmit_due(500).unwrap().unwrap();
    assert_eq!(retry.data, b"heapless");
    sock.on_ack(0, BoundedAckState {
        ack_base: 0,
        receive_bitmap: 0,
        receive_credit: 4,
    }).unwrap();
    assert!(sock.retransmit_due(1000).unwrap().is_none());

    let ack = sock.receive(0, Some(0), b"reply", false).unwrap().unwrap();
    assert_eq!(ack.ack_base, 0);
    let mut out = [0u8; 32];
    let message = sock.recv_into(0, &mut out).unwrap().unwrap();
    assert_eq!(&out[..message.len], b"reply");

    sock.reset_stream(0).unwrap();
    assert!(sock.send(0, b"after-reset", false, 1001).is_err());
}

#[test]
fn socket_set_is_strictly_bounded_and_generation_safe() {
    let mut slots = [SocketStorage::<u32>::EMPTY];
    let mut set = SocketSet::new(&mut slots);
    let first = set.add(1).unwrap();
    assert!(set.add(2).is_err());
    assert_eq!(set.remove(first).unwrap(), 1);
    let second = set.add(2).unwrap();
    assert_ne!(first, second);
    assert!(set.get(first).is_err());
    assert_eq!(*set.get(second).unwrap(), 2);
}
