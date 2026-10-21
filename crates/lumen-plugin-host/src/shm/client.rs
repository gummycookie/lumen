//! Lumen's end of the audio path. Everything here may be called from the
//! audio thread except [`BlockLink::create`]: no allocation, no lock. The one
//! thing that can block is a [`Waiting`] other than `Spin` on the answer, and
//! it is bounded by the deadline the caller gives.

use std::io;
use std::time::Instant;

use super::region::{Answer, FLAG_FLUSH, Layout, Outcome, Region, Stamp, SubmitError};
use super::sys::{Names, Signal};
use super::{Waiting, Wake};
use crate::protocol::{Event, ParamQueues};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Nothing is in flight.
    Idle,
    /// The host has not answered yet.
    Pending,
    /// The host answered; the output can be copied until the next submit.
    Done(Answer),
}

struct InFlight {
    seq: u64,
    generation: u32,
    frames: u32,
}

pub struct BlockLink {
    region: Region,
    go: Signal,
    done: Signal,
    wake: Wake,
    next_seq: u64,
    in_flight: Option<InFlight>,
    /// Length of the block whose answer was last taken, for `copy_output`.
    answered_frames: u32,
    stale: u32,
    invalid: u32,
    /// The next block asks the plugin to forget what it holds.
    flush_next: bool,
}

impl BlockLink {
    /// Makes the shared region and both signals. Not real-time safe. The
    /// names must be handed to the host, then removed with
    /// [`super::sys::remove_names`].
    pub fn create(base: &str, layout: Layout, wake: Wake) -> io::Result<(Self, Names)> {
        let names = Names::new(base);
        let region = Region::create(&names.region, layout)?;
        let go = Signal::create(&names.go)?;
        let done = Signal::create(&names.done)?;
        let link = Self {
            region,
            go,
            done,
            wake,
            next_seq: 1,
            in_flight: None,
            answered_frames: 0,
            stale: 0,
            invalid: 0,
            flush_next: false,
        };
        Ok((link, names))
    }

    pub fn layout(&self) -> &Layout {
        self.region.layout()
    }

    pub fn wake(&self) -> Wake {
        self.wake
    }

    /// Whether the host's block loop is running.
    pub fn host_ready(&self) -> bool {
        self.region.ready()
    }

    /// Answers dropped because they arrived after the block was given up on,
    /// or belonged to another configuration.
    pub fn stale(&self) -> u32 {
        self.stale
    }

    /// Answers that did not match the block asked for, or that the host
    /// wrote wrongly.
    pub fn invalid(&self) -> u32 {
        self.invalid
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Asks the plugin to forget what it holds (its tail, its filters)
    /// before the next block it processes. Allocation-free.
    pub fn flush_next(&mut self) {
        self.flush_next = true;
    }

    /// Hands the host a block and returns its sequence number. Fails with
    /// `Busy` while the host is still on an earlier one; the caller then uses
    /// silence for this block.
    pub fn submit<Q: ParamQueues + ?Sized>(
        &mut self,
        generation: u32,
        frames: u32,
        input: &[&[f32]],
        events: &[Event],
        params: &Q,
    ) -> Result<u64, SubmitError> {
        if let Some(flight) = &self.in_flight {
            if self.region.done() == flight.seq {
                // The host finished a block that was given up on: its answer
                // is stale, and the buffers are free again.
                self.stale += 1;
                self.in_flight = None;
            } else {
                return Err(SubmitError::Busy);
            }
        }
        let seq = self.next_seq;
        let stamp = Stamp {
            seq,
            generation,
            flags: if self.flush_next { FLAG_FLUSH } else { 0 },
        };
        self.region
            .write_flagged(stamp, frames, input, events, params)?;
        self.flush_next = false;
        self.next_seq += 1;
        self.in_flight = Some(InFlight {
            seq,
            generation,
            frames,
        });
        if self.wake.host.needs_signal() {
            self.go.set();
        }
        Ok(seq)
    }

    /// Checks for the answer without waiting.
    pub fn poll(&mut self) -> Poll {
        let Some(flight) = &self.in_flight else {
            return Poll::Idle;
        };
        if self.region.done() != flight.seq {
            return Poll::Pending;
        }
        let (seq, generation, frames) = (flight.seq, flight.generation, flight.frames);
        self.in_flight = None;
        let answer = match self.region.answer() {
            Some(a) if a.seq == seq && a.generation == generation => a,
            // The host wrote an answer that is not for this block.
            _ => {
                self.invalid += 1;
                Answer {
                    seq,
                    generation,
                    outcome: Outcome::Invalid,
                    status: 0,
                    latency: 0,
                    restart: 0,
                    repaired: 0,
                    work_ns: 0,
                }
            }
        };
        if answer.outcome == Outcome::Stale {
            self.stale += 1;
        }
        self.answered_frames = frames;
        Poll::Done(answer)
    }

    /// Waits for the answer until `deadline`, as the link's [`Wake`] says.
    /// Returns `Pending` at the deadline with the block still in flight.
    pub fn wait(&mut self, deadline: Instant) -> Poll {
        let spin_until = match self.wake.lumen {
            Waiting::Spin => deadline,
            Waiting::Event => Instant::now(),
            Waiting::Hybrid(spin) => (Instant::now() + spin).min(deadline),
        };
        loop {
            let poll = self.poll();
            if poll != Poll::Pending {
                return poll;
            }
            if Instant::now() >= spin_until {
                break;
            }
            std::hint::spin_loop();
        }
        if self.wake.lumen == Waiting::Spin {
            return Poll::Pending;
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return self.poll();
            }
            // A signal left over from a late earlier block just loops.
            self.done.wait(left);
            let poll = self.poll();
            if poll != Poll::Pending {
                return poll;
            }
        }
    }

    /// Copies one channel of the answered block into `dst`; samples that are
    /// not finite become silence. Returns how many were replaced.
    pub fn copy_output(&self, channel: usize, dst: &mut [f32]) -> u32 {
        self.region.copy_output(channel, self.answered_frames, dst)
    }
}
