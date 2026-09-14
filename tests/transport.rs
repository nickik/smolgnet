use smolgnet::*;
use smolgnet::gts::{ReliableRx, ReliableTx};

#[test]
fn reliable_window_reorders_and_reports_credit() {
    let mut rx = ReliableRx::new(4);
    assert_eq!(rx.credit(), 4);
    rx.receive(1, b"b".to_vec()).unwrap();
    assert_eq!(rx.credit(), 3);
    rx.receive(0, b"a".to_vec()).unwrap();
    assert_eq!(rx.recv(), Some(b"a".to_vec()));
    assert_eq!(rx.recv(), Some(b"b".to_vec()));
    let (base, bitmap, credit) = rx.ack().unwrap();
    assert_eq!(base, 1);
    assert_eq!(bitmap, 0);
    assert_eq!(credit, 4);
}

#[test]
fn reliable_sender_credit_ack_and_retransmit() {
    let mut tx = ReliableTx::new(1);
    assert_eq!(tx.queue(b"a".to_vec(), false, 0).unwrap(), 0);
    assert_eq!(tx.queue(b"b".to_vec(), false, 0).unwrap_err(), Error::WouldBlock);
    assert!(tx.due(499).is_empty());
    assert_eq!(tx.due(500).len(), 1);
    tx.on_ack(0, 0, 2);
    assert_eq!(tx.outstanding(), 0);
    assert_eq!(tx.peer_credit(), 2);
}

#[test]
fn fixed_reliable_stream_requires_exact_normal_message_size() {
    let profile = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    let mut stream = GtsStream::new(0, profile, true, 2, 2).unwrap();

    assert!(stream
        .send_packet(7, vec![0x11; 18], false, 0)
        .is_ok());
    assert_eq!(
        stream
            .send_packet(7, vec![0x22; 17], false, 1)
            .unwrap_err(),
        Error::InvalidLength
    );
}

#[test]
fn fixed_reliable_stream_allows_short_final_data_end() {
    let profile = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    let mut stream = GtsStream::new(0, profile, true, 2, 2).unwrap();
    let packet = stream.send_packet(7, b"final".to_vec(), true, 0).unwrap();
    assert!(matches!(packet, GtsPacket::Data { end: true, .. }));
    assert_eq!(
        stream.send_packet(7, vec![0; 18], false, 1).unwrap_err(),
        Error::InvalidState
    );
}

#[test]
fn incoming_fixed_packet_must_use_negotiated_size_class() {
    let profile = StreamProfile::reliable_fixed(SizeClass::Ctrl32, Direction::Bidirectional);
    let stream = GtsStream::new(0, profile, false, 4, 4).unwrap();
    let packet = GtsPacket::Data {
        tunnel_id: 7,
        stream_id: 0,
        sequence: 0,
        data: vec![0; 18],
        end: false,
    };
    assert_eq!(
        stream
            .validate_incoming(SizeClass::Ctrl64, &packet)
            .unwrap_err(),
        Error::ProfileViolation
    );
    assert!(stream
        .validate_incoming(SizeClass::Ctrl32, &packet)
        .is_ok());
}

#[test]
fn stream_reset_discards_queued_receive_and_transmit_state() {
    let profile = StreamProfile::reliable_variable(SizeClass::Msg128, Direction::Bidirectional);
    let mut stream = GtsStream::new(0, profile, true, 2, 2).unwrap();
    stream
        .receive_packet(&GtsPacket::Data {
            tunnel_id: 1,
            stream_id: 0,
            sequence: 0,
            data: b"queued".to_vec(),
            end: false,
        })
        .unwrap();
    stream.send_packet(1, b"pending".to_vec(), false, 0).unwrap();
    assert_eq!(stream.recv(), Some(b"queued".to_vec()));

    stream
        .receive_packet(&GtsPacket::Data {
            tunnel_id: 1,
            stream_id: 0,
            sequence: 1,
            data: b"discard me".to_vec(),
            end: false,
        })
        .unwrap();
    stream.reset();
    assert_eq!(stream.state, StreamState::Reset);
    assert_eq!(stream.recv(), None);
    assert_eq!(stream.peer_credit(), Some(0));
    assert!(stream.retransmit_due(1, 10_000).is_empty());
}

#[test]
fn unreliable_stream_rejects_ack_and_reliable_data() {
    let profile = StreamProfile::unreliable_variable(
        SizeClass::Ctrl64,
        Direction::Bidirectional,
        true,
        false,
    );
    let stream = GtsStream::new(1, profile, true, 0, 0).unwrap();
    let ack = GtsPacket::Ack {
        tunnel_id: 1,
        stream_id: 1,
        ack_base: 0,
        receive_bitmap: 0,
        receive_credit: 0,
    };
    assert_eq!(
        stream.validate_incoming(SizeClass::Ctrl32, &ack).unwrap_err(),
        Error::ProfileViolation
    );
}
