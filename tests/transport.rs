use smolgnet::*;
use smolgnet::gts::{ReliableRx,ReliableTx};

#[test]
fn reliable_window_reorders_and_reports_credit() {
    let mut rx=ReliableRx::new(4);assert_eq!(rx.credit(),4);rx.receive(1,b"b".to_vec()).unwrap();assert_eq!(rx.credit(),3);rx.receive(0,b"a".to_vec()).unwrap();assert_eq!(rx.recv(),Some(b"a".to_vec()));assert_eq!(rx.recv(),Some(b"b".to_vec()));let (base,bitmap,credit)=rx.ack().unwrap();assert_eq!(base,1);assert_eq!(bitmap,0);assert_eq!(credit,4);
}

#[test]
fn reliable_sender_credit_ack_and_retransmit() {
    let mut tx=ReliableTx::new(1);assert_eq!(tx.queue(b"a".to_vec(),false,0).unwrap(),0);assert_eq!(tx.queue(b"b".to_vec(),false,0).unwrap_err(),Error::WouldBlock);assert!(tx.due(499).is_empty());assert_eq!(tx.due(500).len(),1);tx.on_ack(0,0,2);assert_eq!(tx.outstanding(),0);assert_eq!(tx.peer_credit(),2);
}
