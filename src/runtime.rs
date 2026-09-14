use crate::endpoint::{Endpoint, TunnelHandle};
use crate::error::Result;
use crate::gts::DEFAULT_RTO_MS;
use crate::link::{Flit, GnetFrame};
use crate::time::{Duration, Instant, PollAt};

/// Typed-time extension methods for the alloc-backed endpoint.
///
/// Existing millisecond APIs remain available for source compatibility. New
/// scheduler/runtime integrations should prefer these typed methods.
pub trait EndpointRuntimeExt {
    fn send_at(
        &mut self,
        tunnel: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: Instant,
    ) -> Result<()>;
    fn send_end_at(
        &mut self,
        tunnel: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: Instant,
    ) -> Result<()>;
    fn receive_flit_at(&mut self, flit: Flit, now: Instant) -> Result<bool>;
    fn receive_frame_at(&mut self, frame: GnetFrame, now: Instant) -> Result<bool>;
    fn tick_at(&mut self, now: Instant) -> Result<usize>;
    fn poll_at(&self, now: Instant) -> PollAt;
    fn poll_delay(&self, now: Instant) -> Option<Duration>;
}

impl EndpointRuntimeExt for Endpoint {
    fn send_at(
        &mut self,
        tunnel: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: Instant,
    ) -> Result<()> {
        self.send(tunnel, stream_id, data, now.total_millis())
    }

    fn send_end_at(
        &mut self,
        tunnel: TunnelHandle,
        stream_id: u8,
        data: &[u8],
        now: Instant,
    ) -> Result<()> {
        self.send_end(tunnel, stream_id, data, now.total_millis())
    }

    fn receive_flit_at(&mut self, flit: Flit, now: Instant) -> Result<bool> {
        self.receive_flit(flit, now.total_millis())
    }

    fn receive_frame_at(&mut self, frame: GnetFrame, now: Instant) -> Result<bool> {
        self.receive_frame(frame, now.total_millis())
    }

    fn tick_at(&mut self, now: Instant) -> Result<usize> {
        self.tick(now.total_millis())
    }

    fn poll_at(&self, now: Instant) -> PollAt {
        if self.dlp().queued_flits() != 0 {
            PollAt::Now
        } else {
            // The existing alloc-backed GTS implementation currently exposes
            // its RTO but not each stream's absolute deadline. Waking once per
            // RTO is conservative and correct; ingress may wake the runtime
            // earlier. When routing work touches the GTS scheduler, this can
            // be refined to the minimum outstanding stream deadline without
            // changing the runtime API.
            PollAt::Time(now + Duration::from_millis(DEFAULT_RTO_MS))
        }
    }

    fn poll_delay(&self, now: Instant) -> Option<Duration> {
        self.poll_at(now).delay_from(now)
    }
}

#[cfg(feature = "async")]
mod async_support {
    use core::ops::{Deref, DerefMut};
    use core::task::Waker;

    use super::{Duration, Endpoint, EndpointRuntimeExt, Flit, GnetFrame, Instant, PollAt, Result, TunnelHandle};

    #[derive(Debug, Default)]
    pub struct WakerRegistration {
        waker: Option<Waker>,
    }

    impl WakerRegistration {
        pub const fn new() -> Self { Self { waker: None } }

        pub fn register(&mut self, waker: &Waker) {
            if self.waker.as_ref().is_some_and(|old| old.will_wake(waker)) {
                return;
            }
            self.waker = Some(waker.clone());
        }

        pub fn wake(&mut self) {
            if let Some(waker) = self.waker.take() {
                waker.wake();
            }
        }

        pub fn clear(&mut self) { self.waker = None; }
    }

    /// Endpoint wrapper with runtime-facing RX/TX waker registration.
    ///
    /// Incoming traffic wakes both registrations because one packet can make
    /// received data available and can also release sender credit/ACK state.
    #[derive(Debug)]
    pub struct AsyncEndpoint {
        endpoint: Endpoint,
        rx_waker: WakerRegistration,
        tx_waker: WakerRegistration,
    }

    impl AsyncEndpoint {
        pub fn new(endpoint: Endpoint) -> Self {
            Self {
                endpoint,
                rx_waker: WakerRegistration::new(),
                tx_waker: WakerRegistration::new(),
            }
        }

        pub fn into_inner(self) -> Endpoint { self.endpoint }

        pub fn register_recv_waker(&mut self, waker: &Waker) { self.rx_waker.register(waker); }
        pub fn register_send_waker(&mut self, waker: &Waker) { self.tx_waker.register(waker); }

        pub fn receive_frame_at(&mut self, frame: GnetFrame, now: Instant) -> Result<bool> {
            let result = self.endpoint.receive_frame_at(frame, now);
            if result.is_ok() {
                self.rx_waker.wake();
                self.tx_waker.wake();
            }
            result
        }

        pub fn receive_flit_at(&mut self, flit: Flit, now: Instant) -> Result<bool> {
            let result = self.endpoint.receive_flit_at(flit, now);
            if result.is_ok() {
                self.rx_waker.wake();
                self.tx_waker.wake();
            }
            result
        }

        pub fn recv(&mut self, tunnel: TunnelHandle, stream_id: u8) -> Result<Option<alloc::vec::Vec<u8>>> {
            let result = self.endpoint.recv(tunnel, stream_id)?;
            if result.is_some() {
                // Consuming a reliable message may queue a fresh receive-credit ACK.
                self.tx_waker.wake();
            }
            Ok(result)
        }

        pub fn send_at(
            &mut self,
            tunnel: TunnelHandle,
            stream_id: u8,
            data: &[u8],
            now: Instant,
        ) -> Result<()> {
            self.endpoint.send_at(tunnel, stream_id, data, now)
        }

        pub fn tick_at(&mut self, now: Instant) -> Result<usize> {
            let count = self.endpoint.tick_at(now)?;
            if count != 0 { self.tx_waker.wake(); }
            Ok(count)
        }

        pub fn poll_at(&self, now: Instant) -> PollAt { self.endpoint.poll_at(now) }
        pub fn poll_delay(&self, now: Instant) -> Option<Duration> { self.endpoint.poll_delay(now) }
    }

    impl Deref for AsyncEndpoint {
        type Target = Endpoint;
        fn deref(&self) -> &Self::Target { &self.endpoint }
    }

    impl DerefMut for AsyncEndpoint {
        fn deref_mut(&mut self) -> &mut Self::Target { &mut self.endpoint }
    }
}

#[cfg(feature = "async")]
pub use self::async_support::{AsyncEndpoint, WakerRegistration};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EndpointConfig, GdpAddress};

    #[test]
    fn idle_endpoint_has_conservative_timer_deadline() {
        let ep = Endpoint::new(GdpAddress(1), EndpointConfig::new(128)).unwrap();
        let now = Instant::from_millis(1000);
        assert_eq!(ep.poll_delay(now), Some(Duration::from_millis(DEFAULT_RTO_MS)));
    }

    #[cfg(feature = "async")]
    #[test]
    fn waker_registration_is_one_shot_and_wakes() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::task::{Wake, Waker};

        struct Counter(AtomicUsize);
        impl Wake for Counter {
            fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::SeqCst); }
        }

        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());
        let mut reg = WakerRegistration::new();
        reg.register(&waker);
        reg.register(&waker);
        reg.wake();
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        reg.wake();
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }
}
