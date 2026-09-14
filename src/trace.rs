use crate::dlp::{GnetFrame, LinkTraffic, Vcid};
use crate::error::Result;
use crate::qdx::GnetFrameDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceDirection {
    Transmit,
    Receive,
}

#[derive(Debug, Clone, Copy)]
pub struct TraceEvent<'a> {
    pub direction: TraceDirection,
    pub vcid: Vcid,
    pub traffic: LinkTraffic,
    pub equivalent_flits: usize,
    pub bytes: &'a [u8],
}

pub trait TraceSink {
    fn trace(&mut self, event: TraceEvent<'_>);
}

impl<F> TraceSink for F
where
    F: for<'a> FnMut(TraceEvent<'a>),
{
    fn trace(&mut self, event: TraceEvent<'_>) {
        self(event);
    }
}

/// QDX/frame-device wrapper that observes traffic without modifying it.
#[derive(Debug)]
pub struct Tracer<D, S> {
    inner: D,
    sink: S,
}

impl<D, S> Tracer<D, S> {
    pub const fn new(inner: D, sink: S) -> Self {
        Self { inner, sink }
    }

    pub fn inner(&self) -> &D {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut D {
        &mut self.inner
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    pub fn into_parts(self) -> (D, S) {
        (self.inner, self.sink)
    }

    fn event<'a>(direction: TraceDirection, frame: &'a GnetFrame) -> TraceEvent<'a> {
        TraceEvent {
            direction,
            vcid: frame.vcid,
            traffic: frame.traffic,
            equivalent_flits: frame.flit_len(),
            bytes: &frame.bytes,
        }
    }
}

impl<D: GnetFrameDevice, S: TraceSink> GnetFrameDevice for Tracer<D, S> {
    fn transmit_frame(&mut self, frame: GnetFrame) -> Result<()> {
        self.sink
            .trace(Self::event(TraceDirection::Transmit, &frame));
        self.inner.transmit_frame(frame)
    }

    fn receive_frame(&mut self) -> Result<Option<GnetFrame>> {
        let frame = self.inner.receive_frame()?;
        if let Some(ref frame) = frame {
            self.sink
                .trace(Self::event(TraceDirection::Receive, frame));
        }
        Ok(frame)
    }
}
