//! The shared block: its layout, and the one place that reads and writes it.
//!
//! One block is in flight at a time, so there is one set of buffers. Lumen
//! writes the request, then publishes it by storing the block's sequence
//! number in `request` (release); the host reads the request after loading it
//! (acquire), writes the answer, then publishes it by storing the same number
//! in `done` (release). Lumen does not touch the buffers between those two
//! points, and the host does not touch them outside them. Everything either
//! side reads from here is untrusted: counts are checked against the layout,
//! never taken on faith, and audio read back is scrubbed of values that are
//! not finite. A host that breaks the hand-off can spoil its own audio and
//! nothing else.

use std::io;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

use super::sys::Mapping;
use crate::instance::Buffers;
use crate::protocol::{
    Event, EventKind, MAX_CHANNELS, MAX_EVENTS, MAX_FRAMES, MAX_POINTS, MAX_QUEUES, ParamQueue,
    ParamQueues,
};

const MAGIC: u32 = 0x4c42_4b31; // "LBK1"
/// Bumped when the layout changes shape.
const VERSION: u32 = 4;

/// A block flag: the plugin forgets what it holds (its tail, its filters)
/// before it processes this block. Sent by a jump in playback.
pub const FLAG_FLUSH: u32 = 1;

/// What names a block rather than fills it: its number, the configuration
/// it was made for and its flags (`FLAG_*`).
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub seq: u64,
    pub generation: u32,
    pub flags: u32,
}

/// The capacity of a block. Fixed when the region is made; a larger need
/// means a new region, which happens off the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub max_frames: u32,
    /// Channels of the main input and output buses; 0 for none.
    pub inputs: u8,
    pub outputs: u8,
    pub max_events: u32,
    pub max_queues: u32,
    pub max_points: u32,
}

impl Layout {
    /// A layout with room for typical blocks: every note of a dense chord
    /// and a few hundred automation points.
    pub fn new(max_frames: u32, inputs: u8, outputs: u8) -> Self {
        Self {
            max_frames,
            inputs,
            outputs,
            max_events: 512,
            max_queues: 64,
            max_points: 1024,
        }
    }

    /// Whether the layout is inside the protocol's limits.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.max_frames == 0 || self.max_frames as usize > MAX_FRAMES {
            return Err("frame capacity");
        }
        if usize::from(self.inputs) > MAX_CHANNELS || usize::from(self.outputs) > MAX_CHANNELS {
            return Err("channel count");
        }
        if self.max_events as usize > MAX_EVENTS
            || self.max_queues as usize > MAX_QUEUES
            || self.max_points as usize > MAX_POINTS
        {
            return Err("event or parameter capacity");
        }
        Ok(())
    }
}

/// How a block ended, as the host reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Outcome {
    /// Processed; the output is valid.
    Done = 0,
    /// The block was for another configuration; nothing was processed.
    Stale = 1,
    /// The block did not fit the plugin's format or the layout.
    Invalid = 2,
    /// The plugin failed the call.
    Failed = 3,
    /// No plugin is configured to process it.
    NotReady = 4,
}

impl Outcome {
    pub(super) fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Done,
            1 => Self::Stale,
            2 => Self::Invalid,
            3 => Self::Failed,
            4 => Self::NotReady,
            _ => return None,
        })
    }
}

/// What the host recorded about a finished block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub seq: u64,
    pub generation: u32,
    pub outcome: Outcome,
    pub status: i32,
    pub latency: u32,
    pub restart: u32,
    pub repaired: u32,
    /// How long the plugin worked on the block, in nanoseconds (0 when it
    /// did not process it).
    pub work_ns: u32,
}

/// Why a block was not submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// The host is still on an earlier block.
    Busy,
    /// The block does not fit the layout or is malformed.
    Invalid(&'static str),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Busy => write!(f, "the host is still on the previous block"),
            SubmitError::Invalid(what) => write!(f, "the block does not fit: {what}"),
        }
    }
}

impl std::error::Error for SubmitError {}

/// A block the host could not trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadBlock(pub &'static str);

#[repr(C, align(64))]
struct Line<T>(T);

#[derive(Default)]
#[repr(C)]
struct Ident {
    magic: AtomicU32,
    version: AtomicU32,
    max_frames: AtomicU32,
    inputs: AtomicU32,
    outputs: AtomicU32,
    events: AtomicU32,
    queues: AtomicU32,
    points: AtomicU32,
}

#[repr(C)]
struct Meta {
    generation: AtomicU32,
    frames: AtomicU32,
    events: AtomicU32,
    queues: AtomicU32,
    points: AtomicU32,
    flags: AtomicU32,
}

#[repr(C)]
struct Record {
    seq: AtomicU64,
    generation: AtomicU32,
    outcome: AtomicU32,
    status: AtomicI32,
    latency: AtomicU32,
    restart: AtomicU32,
    repaired: AtomicU32,
    work_ns: AtomicU32,
}

/// The control words, each on a cache line of its own so that the two
/// processes do not share a line they both write.
#[repr(C)]
struct Control {
    ident: Line<Ident>,
    request: Line<AtomicU64>,
    done: Line<AtomicU64>,
    ready: Line<AtomicU32>,
    quit: Line<AtomicU32>,
    meta: Line<Meta>,
    record: Line<Record>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawEvent {
    offset: u32,
    tag: u32,
    channel: i32,
    pitch: i32,
    velocity: f32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawQueue {
    id: u32,
    points: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawPoint {
    offset: u32,
    _pad: u32,
    value: f64,
}

fn align(n: usize) -> usize {
    n.next_multiple_of(64)
}

/// Where each array starts, in bytes from the region's start.
struct Offsets {
    input: usize,
    events: usize,
    queues: usize,
    points: usize,
    output: usize,
    total: usize,
}

impl Offsets {
    fn of(layout: &Layout) -> Self {
        let samples = |channels: u8| align(usize::from(channels) * layout.max_frames as usize * 4);
        let input = size_of::<Control>();
        let events = input + samples(layout.inputs);
        let queues = events + align(layout.max_events as usize * size_of::<RawEvent>());
        let points = queues + align(layout.max_queues as usize * size_of::<RawQueue>());
        let output = points + align(layout.max_points as usize * size_of::<RawPoint>());
        Self {
            input,
            events,
            queues,
            points,
            output,
            total: output + samples(layout.outputs),
        }
    }
}

/// The reusable buffers the host fills from a request and processes in.
/// Made once per link, so a steady stream of blocks allocates nothing.
pub struct BlockScratch {
    pub buffers: Buffers,
    events: Vec<Event>,
    queues: Vec<ParamQueue>,
    used: usize,
    /// The block read last asked the plugin to forget what it holds.
    pub flush: bool,
}

impl BlockScratch {
    pub fn new(layout: &Layout) -> Self {
        let frames = layout.max_frames as usize;
        Self {
            buffers: Buffers {
                input: vec![vec![0.0; frames]; usize::from(layout.inputs)],
                output: vec![vec![0.0; frames]; usize::from(layout.outputs)],
            },
            events: Vec::with_capacity(layout.max_events as usize),
            queues: Vec::new(),
            used: 0,
            flush: false,
        }
    }

    /// The block read last, ready for [`crate::Instance::process_into`]:
    /// the buffers to lend, and the events and parameter changes to give.
    pub fn parts(&mut self) -> (&mut Buffers, &[Event], &[ParamQueue]) {
        (&mut self.buffers, &self.events, &self.queues[..self.used])
    }
}

/// A mapped region of shared memory with this layout.
pub struct Region {
    map: Mapping,
    layout: Layout,
    at: Offsets,
}

impl Region {
    /// Makes a region and stamps it with its layout. Lumen's side.
    pub fn create(name: &str, layout: Layout) -> io::Result<Self> {
        layout.check().map_err(io::Error::other)?;
        let at = Offsets::of(&layout);
        let map = Mapping::create(name, at.total)?;
        let region = Self { map, layout, at };
        let id = &region.control().ident.0;
        id.max_frames.store(layout.max_frames, Ordering::Relaxed);
        id.inputs.store(u32::from(layout.inputs), Ordering::Relaxed);
        id.outputs
            .store(u32::from(layout.outputs), Ordering::Relaxed);
        id.events.store(layout.max_events, Ordering::Relaxed);
        id.queues.store(layout.max_queues, Ordering::Relaxed);
        id.points.store(layout.max_points, Ordering::Relaxed);
        id.version.store(VERSION, Ordering::Relaxed);
        id.magic.store(MAGIC, Ordering::Release);
        Ok(region)
    }

    /// Maps a region made by Lumen and checks that it is the one described.
    pub fn open(name: &str, layout: Layout) -> io::Result<Self> {
        layout.check().map_err(io::Error::other)?;
        let at = Offsets::of(&layout);
        let map = Mapping::open(name, at.total)?;
        let region = Self { map, layout, at };
        let id = &region.control().ident.0;
        let same = id.magic.load(Ordering::Acquire) == MAGIC
            && id.version.load(Ordering::Relaxed) == VERSION
            && id.max_frames.load(Ordering::Relaxed) == layout.max_frames
            && id.inputs.load(Ordering::Relaxed) == u32::from(layout.inputs)
            && id.outputs.load(Ordering::Relaxed) == u32::from(layout.outputs)
            && id.events.load(Ordering::Relaxed) == layout.max_events
            && id.queues.load(Ordering::Relaxed) == layout.max_queues
            && id.points.load(Ordering::Relaxed) == layout.max_points;
        if !same {
            return Err(io::Error::other(
                "the shared region is not the one described",
            ));
        }
        Ok(region)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn control(&self) -> &Control {
        // SAFETY: the mapping is at least `size_of::<Control>()` bytes, page
        // aligned, and every field is an atomic, valid for any bit pattern.
        unsafe { &*self.map.as_ptr().cast::<Control>() }
    }

    fn array<T>(&self, offset: usize) -> *mut T {
        debug_assert!(offset <= self.at.total);
        // SAFETY: `offset` is one of the computed starts, inside the mapping.
        unsafe { self.map.as_ptr().add(offset).cast() }
    }

    // --- control words ---

    pub fn request(&self) -> u64 {
        self.control().request.0.load(Ordering::Acquire)
    }

    pub fn done(&self) -> u64 {
        self.control().done.0.load(Ordering::Acquire)
    }

    pub fn quitting(&self) -> bool {
        self.control().quit.0.load(Ordering::Acquire) != 0
    }

    pub(super) fn set_quit(&self) {
        self.control().quit.0.store(1, Ordering::Release);
    }

    pub fn ready(&self) -> bool {
        self.control().ready.0.load(Ordering::Acquire) != 0
    }

    pub fn set_ready(&self) {
        self.control().ready.0.store(1, Ordering::Release);
    }

    // --- Lumen's side ---

    /// Writes a block and publishes it as `seq`. Allocation-free.
    pub fn write_request<Q: ParamQueues + ?Sized>(
        &self,
        seq: u64,
        generation: u32,
        frames: u32,
        input: &[&[f32]],
        events: &[Event],
        params: &Q,
    ) -> Result<(), SubmitError> {
        let stamp = Stamp {
            seq,
            generation,
            flags: 0,
        };
        self.write_flagged(stamp, frames, input, events, params)
    }

    /// As [`Self::write_request`], with block flags ([`FLAG_FLUSH`]).
    pub fn write_flagged<Q: ParamQueues + ?Sized>(
        &self,
        stamp: Stamp,
        frames: u32,
        input: &[&[f32]],
        events: &[Event],
        params: &Q,
    ) -> Result<(), SubmitError> {
        let Stamp {
            seq,
            generation,
            flags,
        } = stamp;
        let layout = &self.layout;
        if frames == 0 || frames > layout.max_frames {
            return Err(SubmitError::Invalid("frame count"));
        }
        let n = frames as usize;
        if input.len() != usize::from(layout.inputs) || input.iter().any(|c| c.len() < n) {
            return Err(SubmitError::Invalid("input channels"));
        }
        if events.len() > layout.max_events as usize {
            return Err(SubmitError::Invalid("too many events"));
        }
        if events.iter().any(|e| e.offset >= frames) {
            return Err(SubmitError::Invalid("event offset"));
        }
        let queues = params.queue_count();
        if queues > layout.max_queues as usize {
            return Err(SubmitError::Invalid("too many parameter queues"));
        }
        let mut points = 0_usize;
        for i in 0..queues {
            let (_, list) = params.queue(i);
            points += list.len();
            if list.iter().any(|&(offset, _)| offset >= frames) {
                return Err(SubmitError::Invalid("point offset"));
            }
            if list.iter().any(|&(_, value)| !value.is_finite()) {
                return Err(SubmitError::Invalid("point value"));
            }
        }
        if points > layout.max_points as usize {
            return Err(SubmitError::Invalid("too many parameter points"));
        }

        let max = layout.max_frames as usize;
        let samples: *mut f32 = self.array(self.at.input);
        for (channel, data) in input.iter().enumerate() {
            // SAFETY: channel `c` starts at `c * max` samples and `n <= max`,
            // inside the input array; the host is not reading (the caller
            // submits only when the previous block is done).
            unsafe { ptr::copy_nonoverlapping(data.as_ptr(), samples.add(channel * max), n) };
        }
        let raw_events: *mut RawEvent = self.array(self.at.events);
        for (i, event) in events.iter().enumerate() {
            let (tag, channel, pitch, velocity) = event.kind.parts();
            // SAFETY: `i < max_events`, checked above.
            unsafe {
                raw_events.add(i).write(RawEvent {
                    offset: event.offset,
                    tag: u32::from(tag),
                    channel: i32::from(channel),
                    pitch: i32::from(pitch),
                    velocity,
                    _pad: 0,
                });
            }
        }
        let raw_queues: *mut RawQueue = self.array(self.at.queues);
        let raw_points: *mut RawPoint = self.array(self.at.points);
        let mut next = 0;
        for i in 0..queues {
            let (id, list) = params.queue(i);
            // SAFETY: `i < max_queues` and `next + len <= max_points`, checked above.
            unsafe {
                raw_queues.add(i).write(RawQueue {
                    id,
                    points: list.len() as u32,
                });
                for &(offset, value) in list {
                    raw_points.add(next).write(RawPoint {
                        offset,
                        _pad: 0,
                        value,
                    });
                    next += 1;
                }
            }
        }
        let meta = &self.control().meta.0;
        meta.generation.store(generation, Ordering::Relaxed);
        meta.frames.store(frames, Ordering::Relaxed);
        meta.events.store(events.len() as u32, Ordering::Relaxed);
        meta.queues.store(queues as u32, Ordering::Relaxed);
        meta.points.store(points as u32, Ordering::Relaxed);
        meta.flags.store(flags, Ordering::Relaxed);
        self.control().request.0.store(seq, Ordering::Release);
        Ok(())
    }

    /// What the host recorded for the block it finished. Call after `done()`
    /// returned the block's number.
    pub fn answer(&self) -> Option<Answer> {
        let r = &self.control().record.0;
        Some(Answer {
            seq: r.seq.load(Ordering::Relaxed),
            generation: r.generation.load(Ordering::Relaxed),
            outcome: Outcome::from_u32(r.outcome.load(Ordering::Relaxed))?,
            status: r.status.load(Ordering::Relaxed),
            latency: r.latency.load(Ordering::Relaxed),
            restart: r.restart.load(Ordering::Relaxed),
            repaired: r.repaired.load(Ordering::Relaxed),
            work_ns: r.work_ns.load(Ordering::Relaxed),
        })
    }

    /// Copies one output channel of the finished block into `dst`, replacing
    /// anything not finite with silence. Returns how many it replaced.
    pub fn copy_output(&self, channel: usize, frames: u32, dst: &mut [f32]) -> u32 {
        if channel >= usize::from(self.layout.outputs) {
            dst.fill(0.0);
            return 0;
        }
        let n = (frames.min(self.layout.max_frames) as usize).min(dst.len());
        let samples: *const f32 = self.array(self.at.output);
        // SAFETY: the channel starts at `channel * max_frames` and `n` is at
        // most `max_frames`, inside the output array.
        unsafe {
            ptr::copy_nonoverlapping(
                samples.add(channel * self.layout.max_frames as usize),
                dst.as_mut_ptr(),
                n,
            );
        }
        dst[n..].fill(0.0);
        let mut repaired = 0;
        for s in &mut dst[..n] {
            if !s.is_finite() {
                *s = 0.0;
                repaired += 1;
            }
        }
        repaired
    }

    // --- the host's side ---

    /// Reads the request published as the latest into `scratch`, checking
    /// everything. Returns the block's configuration generation and length.
    pub fn read_request(&self, scratch: &mut BlockScratch) -> Result<(u32, u32), BadBlock> {
        let layout = &self.layout;
        let meta = &self.control().meta.0;
        let generation = meta.generation.load(Ordering::Relaxed);
        let frames = meta.frames.load(Ordering::Relaxed);
        if frames == 0 || frames > layout.max_frames {
            return Err(BadBlock("frame count"));
        }
        let n = frames as usize;
        let events = meta.events.load(Ordering::Relaxed) as usize;
        let queues = meta.queues.load(Ordering::Relaxed) as usize;
        let points = meta.points.load(Ordering::Relaxed) as usize;
        scratch.flush = meta.flags.load(Ordering::Relaxed) & FLAG_FLUSH != 0;
        if events > layout.max_events as usize
            || queues > layout.max_queues as usize
            || points > layout.max_points as usize
        {
            return Err(BadBlock("counts above the layout"));
        }

        let max = layout.max_frames as usize;
        let samples: *const f32 = self.array(self.at.input);
        for (channel, data) in scratch.buffers.input.iter_mut().enumerate() {
            // SAFETY: as for the write: inside the input array, and the
            // request has been published, so Lumen is not writing.
            unsafe { ptr::copy_nonoverlapping(samples.add(channel * max), data.as_mut_ptr(), n) };
        }

        scratch.events.clear();
        let raw_events: *const RawEvent = self.array(self.at.events);
        for i in 0..events {
            // SAFETY: `i < events <= max_events`.
            let raw = unsafe { raw_events.add(i).read() };
            if raw.offset >= frames {
                return Err(BadBlock("event offset"));
            }
            let channel = i16::try_from(raw.channel).map_err(|_| BadBlock("event channel"))?;
            let pitch = i16::try_from(raw.pitch).map_err(|_| BadBlock("event pitch"))?;
            let velocity = raw.velocity;
            let tag = u8::try_from(raw.tag).map_err(|_| BadBlock("event kind"))?;
            let kind = EventKind::from_parts(tag, channel, pitch, velocity)
                .ok_or(BadBlock("event kind"))?;
            scratch.events.push(Event {
                offset: raw.offset,
                kind,
            });
        }

        let raw_queues: *const RawQueue = self.array(self.at.queues);
        let raw_points: *const RawPoint = self.array(self.at.points);
        scratch.used = 0;
        let mut next = 0_usize;
        for i in 0..queues {
            // SAFETY: `i < queues <= max_queues`.
            let queue = unsafe { raw_queues.add(i).read() };
            let count = queue.points as usize;
            if count > points - next.min(points) {
                return Err(BadBlock("parameter points"));
            }
            if scratch.used == scratch.queues.len() {
                scratch.queues.push(ParamQueue {
                    id: 0,
                    points: Vec::new(),
                });
            }
            let slot = &mut scratch.queues[scratch.used];
            slot.id = queue.id;
            slot.points.clear();
            for _ in 0..count {
                // SAFETY: `next < points <= max_points`, checked just above.
                let point = unsafe { raw_points.add(next).read() };
                next += 1;
                if point.offset >= frames || !point.value.is_finite() {
                    return Err(BadBlock("parameter point"));
                }
                slot.points.push((point.offset, point.value));
            }
            scratch.used += 1;
        }
        if next != points {
            return Err(BadBlock("parameter point total"));
        }
        Ok((generation, frames))
    }

    /// Records the answer to block `seq` and publishes it. For `Done`, the
    /// first `frames` samples of `output` are copied back first.
    pub fn finish(&self, answer: &Answer, frames: u32, output: &[Vec<f32>]) {
        if answer.outcome == Outcome::Done {
            let max = self.layout.max_frames as usize;
            let n = (frames as usize).min(max);
            let samples: *mut f32 = self.array(self.at.output);
            for (channel, data) in output
                .iter()
                .enumerate()
                .take(usize::from(self.layout.outputs))
            {
                let n = n.min(data.len());
                // SAFETY: inside the output array, as above; Lumen is not reading.
                unsafe { ptr::copy_nonoverlapping(data.as_ptr(), samples.add(channel * max), n) };
            }
        }
        let r = &self.control().record.0;
        r.seq.store(answer.seq, Ordering::Relaxed);
        r.generation.store(answer.generation, Ordering::Relaxed);
        r.outcome.store(answer.outcome as u32, Ordering::Relaxed);
        r.status.store(answer.status, Ordering::Relaxed);
        r.latency.store(answer.latency, Ordering::Relaxed);
        r.restart.store(answer.restart, Ordering::Relaxed);
        r.repaired.store(answer.repaired, Ordering::Relaxed);
        r.work_ns.store(answer.work_ns, Ordering::Relaxed);
        self.control().done.0.store(answer.seq, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shm::sys::{Names, remove_names};
    use std::sync::atomic::AtomicU32 as Counter;

    fn region(layout: Layout) -> (Region, Names) {
        static N: Counter = Counter::new(0);
        let base = format!(
            "r{:x}-{:x}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let names = Names::new(&base);
        let region = Region::create(&names.region, layout).expect("create");
        (region, names)
    }

    fn note(offset: u32) -> Event {
        Event {
            offset,
            kind: EventKind::NoteOn {
                channel: 1,
                pitch: 60,
                velocity: 0.5,
            },
        }
    }

    fn written(r: &Region, scratch: &mut BlockScratch) -> Result<(u32, u32), BadBlock> {
        let left = [0.25_f32; 8];
        let right = [-0.25_f32; 8];
        let params = [ParamQueue {
            id: 9,
            points: vec![(0, 0.1), (4, 0.9)],
        }];
        r.write_request(1, 3, 8, &[&left, &right], &[note(2)], &params)
            .expect("write");
        r.read_request(scratch)
    }

    #[test]
    fn a_request_reads_back_as_written() {
        let layout = Layout::new(16, 2, 2);
        let (r, names) = region(layout);
        let mut scratch = BlockScratch::new(&layout);
        let (generation, frames) = written(&r, &mut scratch).expect("read");
        remove_names(&names);
        assert_eq!((generation, frames), (3, 8));
        let (buffers, events, params) = scratch.parts();
        assert_eq!(buffers.input[0][..8], [0.25; 8]);
        assert_eq!(buffers.input[1][..8], [-0.25; 8]);
        assert_eq!(events, [note(2)]);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].id, 9);
        assert_eq!(params[0].points, vec![(0, 0.1), (4, 0.9)]);
        assert_eq!(r.request(), 1);
    }

    #[test]
    fn a_second_block_replaces_the_first_without_leftovers() {
        let layout = Layout::new(16, 0, 2);
        let (r, names) = region(layout);
        let mut scratch = BlockScratch::new(&layout);
        let many = [
            ParamQueue {
                id: 1,
                points: vec![(0, 0.5); 5],
            },
            ParamQueue {
                id: 2,
                points: vec![(1, 0.5); 3],
            },
        ];
        r.write_request(1, 1, 8, &[], &[note(0), note(1)], &many)
            .expect("write");
        r.read_request(&mut scratch).expect("read");
        r.write_request(2, 1, 8, &[], &[note(3)], &[])
            .expect("write");
        r.read_request(&mut scratch).expect("read");
        remove_names(&names);
        let (_, events, params) = scratch.parts();
        assert_eq!(events, [note(3)]);
        assert!(params.is_empty());
    }

    #[test]
    fn what_does_not_fit_is_refused_before_anything_is_written() {
        let layout = Layout {
            max_events: 2,
            max_queues: 1,
            max_points: 2,
            ..Layout::new(8, 1, 1)
        };
        let (r, names) = region(layout);
        remove_names(&names);
        let ch = [0.0_f32; 8];
        let q = |n: usize, at: u32| ParamQueue {
            id: 1,
            points: vec![(at, 0.5); n],
        };
        let write = |frames, input: &[&[f32]], events: &[Event], params: &[ParamQueue]| {
            r.write_request(1, 1, frames, input, events, params)
        };
        assert_eq!(
            write(0, &[&ch], &[], &[]),
            Err(SubmitError::Invalid("frame count"))
        );
        assert_eq!(
            write(9, &[&ch], &[], &[]),
            Err(SubmitError::Invalid("frame count"))
        );
        assert_eq!(
            write(8, &[], &[], &[]),
            Err(SubmitError::Invalid("input channels"))
        );
        assert_eq!(
            write(8, &[&ch[..4]], &[], &[]),
            Err(SubmitError::Invalid("input channels"))
        );
        assert_eq!(
            write(8, &[&ch], &[note(0), note(1), note(2)], &[]),
            Err(SubmitError::Invalid("too many events"))
        );
        assert_eq!(
            write(8, &[&ch], &[note(8)], &[]),
            Err(SubmitError::Invalid("event offset"))
        );
        assert_eq!(
            write(8, &[&ch], &[], &[q(1, 0), q(1, 0)]),
            Err(SubmitError::Invalid("too many parameter queues"))
        );
        assert_eq!(
            write(8, &[&ch], &[], &[q(3, 0)]),
            Err(SubmitError::Invalid("too many parameter points"))
        );
        assert_eq!(
            write(8, &[&ch], &[], &[q(1, 8)]),
            Err(SubmitError::Invalid("point offset"))
        );
        let not_a_number = ParamQueue {
            id: 1,
            points: vec![(0, f64::NAN)],
        };
        assert_eq!(
            write(8, &[&ch], &[], &[not_a_number]),
            Err(SubmitError::Invalid("point value"))
        );
        // Nothing was published.
        assert_eq!(r.request(), 0);
    }

    #[test]
    fn a_region_of_another_layout_is_refused() {
        let layout = Layout::new(16, 2, 2);
        let (_r, names) = region(layout);
        let other = Layout::new(32, 2, 2);
        assert!(Region::open(&names.region, other).is_err());
        assert!(Region::open(&names.region, layout).is_ok());
        remove_names(&names);
    }

    #[test]
    fn a_layout_outside_the_limits_is_refused() {
        assert!(Layout::new(0, 2, 2).check().is_err());
        assert!(Layout::new(MAX_FRAMES as u32 + 1, 2, 2).check().is_err());
        assert!(Layout::new(64, MAX_CHANNELS as u8 + 1, 2).check().is_err());
        assert!(
            Layout {
                max_points: MAX_POINTS as u32 + 1,
                ..Layout::new(64, 2, 2)
            }
            .check()
            .is_err()
        );
        assert!(Layout::new(MAX_FRAMES as u32, 16, 16).check().is_ok());
    }

    /// The meta words are what Lumen writes, but the host reads them as
    /// untrusted. Each of these is a host or Lumen bug or a scribbling
    /// plugin; none may reach the plugin or read outside the layout.
    #[test]
    fn a_request_that_lies_about_itself_is_rejected() {
        let layout = Layout::new(16, 1, 1);
        let (r, names) = region(layout);
        remove_names(&names);
        let ch = [0.0_f32; 8];
        let mut scratch = BlockScratch::new(&layout);
        let fresh = |r: &Region| {
            r.write_request(
                1,
                1,
                8,
                &[&ch],
                &[note(1)],
                &[ParamQueue {
                    id: 1,
                    points: vec![(2, 0.5)],
                }],
            )
            .expect("write");
        };
        let meta = &r.control().meta.0;

        fresh(&r);
        meta.frames.store(0, Ordering::Relaxed);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("frame count")));
        fresh(&r);
        meta.frames.store(17, Ordering::Relaxed);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("frame count")));
        fresh(&r);
        meta.events.store(layout.max_events + 1, Ordering::Relaxed);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("counts above the layout"))
        );
        fresh(&r);
        meta.queues.store(layout.max_queues + 1, Ordering::Relaxed);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("counts above the layout"))
        );
        fresh(&r);
        meta.points.store(layout.max_points + 1, Ordering::Relaxed);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("counts above the layout"))
        );
        // A queue that claims more points than the block has.
        fresh(&r);
        meta.points.store(0, Ordering::Relaxed);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("parameter points"))
        );
        // A total that does not add up.
        fresh(&r);
        meta.points.store(5, Ordering::Relaxed);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("parameter point total"))
        );
        // Sanity: an honest request still reads.
        fresh(&r);
        assert!(r.read_request(&mut scratch).is_ok());
    }

    #[test]
    fn a_scribbled_event_or_point_is_rejected() {
        let layout = Layout::new(16, 0, 1);
        let (r, names) = region(layout);
        remove_names(&names);
        let mut scratch = BlockScratch::new(&layout);
        let params = [ParamQueue {
            id: 1,
            points: vec![(2, 0.5)],
        }];
        let fresh = || {
            r.write_request(1, 1, 8, &[], &[note(1)], &params)
                .expect("write")
        };
        let event = |change: fn(&mut RawEvent)| {
            // SAFETY: the first event slot is inside the mapping, which lives
            // as long as `r`; no other reference to it exists in this test.
            change(unsafe { &mut *r.array::<RawEvent>(r.at.events) });
        };
        let point = |change: fn(&mut RawPoint)| {
            // SAFETY: as above, for the first parameter point.
            change(unsafe { &mut *r.array::<RawPoint>(r.at.points) });
        };

        fresh();
        event(|e| e.offset = 8);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("event offset")));
        fresh();
        event(|e| e.tag = 7);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("event kind")));
        fresh();
        event(|e| e.channel = 70_000);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("event channel")));
        fresh();
        event(|e| e.pitch = -70_000);
        assert_eq!(r.read_request(&mut scratch), Err(BadBlock("event pitch")));
        fresh();
        point(|p| p.offset = 8);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("parameter point"))
        );
        fresh();
        point(|p| p.value = f64::NAN);
        assert_eq!(
            r.read_request(&mut scratch),
            Err(BadBlock("parameter point"))
        );
    }

    #[test]
    fn output_comes_back_and_is_scrubbed() {
        let layout = Layout::new(8, 0, 2);
        let (r, names) = region(layout);
        remove_names(&names);
        let answer = Answer {
            seq: 4,
            generation: 2,
            outcome: Outcome::Done,
            status: 0,
            latency: 11,
            restart: 0,
            repaired: 0,
            work_ns: 120_000,
        };
        let out = vec![vec![0.5, f32::NAN, 0.25, f32::INFINITY], vec![1.0; 4]];
        r.finish(&answer, 4, &out);
        assert_eq!(r.done(), 4);
        assert_eq!(r.answer(), Some(answer));
        let mut dst = [9.0_f32; 6];
        assert_eq!(r.copy_output(0, 4, &mut dst), 2);
        assert_eq!(dst, [0.5, 0.0, 0.25, 0.0, 0.0, 0.0]);
        assert_eq!(r.copy_output(1, 4, &mut dst), 0);
        assert_eq!(dst[..4], [1.0; 4]);
        // A channel the layout does not have is silence.
        assert_eq!(r.copy_output(2, 4, &mut dst), 0);
        assert_eq!(dst, [0.0; 6]);
    }

    #[test]
    fn an_answer_that_is_not_done_copies_no_audio() {
        let layout = Layout::new(8, 0, 1);
        let (r, names) = region(layout);
        remove_names(&names);
        let answer = Answer {
            seq: 1,
            generation: 1,
            outcome: Outcome::Failed,
            status: -1,
            latency: 0,
            restart: 0,
            repaired: 0,
            work_ns: 0,
        };
        r.finish(&answer, 4, &[vec![1.0; 4]]);
        let mut dst = [7.0_f32; 4];
        r.copy_output(0, 4, &mut dst);
        assert_eq!(dst, [0.0; 4]);
    }
}
