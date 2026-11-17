//! In-memory ring buffer that keeps recent session traffic for late readers.
//!
//! Every chunk gets a monotonically increasing sequence number, so a client can
//! poll with `read_since(last_seq)` and never miss or duplicate data.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::types::Direction;

/// A single piece of session traffic.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub seq: u64,
    pub ts_ms: i64,
    pub dir: Direction,
    pub data: Vec<u8>,
}

/// Result of a `read_since` call.
#[derive(Debug, Clone)]
pub struct ReadBatch {
    pub chunks: Vec<Chunk>,
    /// Pass this back as `since_seq` on the next call.
    pub next_seq: u64,
    /// Number of chunks that were already evicted before the requested cursor.
    pub dropped_chunks: u64,
}

impl ReadBatch {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.data.len()).sum()
    }
}

struct State {
    chunks: VecDeque<Chunk>,
    next_seq: u64,
    bytes: usize,
}

/// Bounded, thread-safe ring buffer of session chunks.
pub struct RingBuffer {
    state: Mutex<State>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                chunks: VecDeque::new(),
                next_seq: 1,
                bytes: 0,
            }),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&self, dir: Direction, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let seq = st.next_seq;
        st.next_seq += 1;
        st.bytes += data.len();
        st.chunks.push_back(Chunk {
            seq,
            ts_ms: crate::util::now_ms(),
            dir,
            data: data.to_vec(),
        });
        while st.bytes > self.capacity && st.chunks.len() > 1 {
            if let Some(front) = st.chunks.pop_front() {
                st.bytes = st.bytes.saturating_sub(front.data.len());
            }
        }
    }

    /// Return chunks with `seq >= since_seq`, up to roughly `max_bytes`.
    /// At least one chunk is returned when data is available.
    ///
    /// `next_seq` is the cursor that follows the last returned chunk, so a
    /// caller that hit the byte limit can resume without losing data.
    pub fn read_since(&self, since_seq: u64, max_bytes: usize) -> ReadBatch {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let first_seq = st.chunks.front().map(|c| c.seq).unwrap_or(st.next_seq);
        let start = since_seq.max(1);
        let dropped_chunks = first_seq.saturating_sub(start);
        let mut chunks = Vec::new();
        let mut taken = 0usize;
        let mut next_seq = st.next_seq;
        for chunk in st.chunks.iter() {
            if chunk.seq < start {
                continue;
            }
            if taken > 0 && taken + chunk.data.len() > max_bytes {
                // Resume here on the next call instead of skipping this chunk.
                next_seq = chunk.seq;
                break;
            }
            taken += chunk.data.len();
            next_seq = chunk.seq + 1;
            chunks.push(chunk.clone());
            if taken >= max_bytes {
                break;
            }
        }
        ReadBatch {
            chunks,
            next_seq,
            dropped_chunks,
        }
    }

    /// Most recent `max_bytes` of traffic.
    pub fn tail(&self, max_bytes: usize) -> ReadBatch {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut taken = 0usize;
        for chunk in st.chunks.iter().rev() {
            if taken > 0 && taken + chunk.data.len() > max_bytes {
                break;
            }
            taken += chunk.data.len();
            chunks.push(chunk.clone());
            if taken >= max_bytes {
                break;
            }
        }
        chunks.reverse();
        ReadBatch {
            chunks,
            next_seq: st.next_seq,
            dropped_chunks: 0,
        }
    }

    pub fn clear(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.chunks.clear();
        st.bytes = 0;
    }

    pub fn len_bytes(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bytes
    }

    pub fn next_seq(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_seq
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_sequence_numbers() {
        let ring = RingBuffer::new(4096);
        ring.push(Direction::Rx, b"hello");
        ring.push(Direction::Tx, b"world");
        let batch = ring.read_since(1, 1024);
        assert_eq!(batch.chunks.len(), 2);
        assert_eq!(batch.chunks[0].seq, 1);
        assert_eq!(batch.next_seq, 3);
        assert!(ring.read_since(3, 1024).is_empty());
    }

    #[test]
    fn evicts_old_data_but_keeps_recent() {
        let ring = RingBuffer::new(8);
        ring.push(Direction::Rx, b"12345678");
        ring.push(Direction::Rx, b"abcdefgh");
        let batch = ring.read_since(1, 1024);
        assert_eq!(batch.dropped_chunks, 1);
        assert_eq!(batch.chunks.len(), 1);
        assert_eq!(batch.chunks[0].data, b"abcdefgh");
    }

    #[test]
    fn tail_returns_latest_bytes() {
        let ring = RingBuffer::new(4096);
        for i in 0..10u8 {
            ring.push(Direction::Rx, &[i]);
        }
        let batch = ring.tail(3);
        assert_eq!(batch.chunks.len(), 3);
        assert_eq!(batch.chunks[2].data, vec![9]);
    }

    #[test]
    fn byte_limit_does_not_skip_chunks() {
        let ring = RingBuffer::new(4096);
        ring.push(Direction::Rx, b"aaaa");
        ring.push(Direction::Rx, b"bbbb");
        ring.push(Direction::Rx, b"cccc");

        let first = ring.read_since(1, 4);
        assert_eq!(first.chunks.len(), 1);
        assert_eq!(first.chunks[0].data, b"aaaa");
        assert_eq!(first.next_seq, 2);

        let second = ring.read_since(first.next_seq, 4);
        assert_eq!(second.chunks.len(), 1);
        assert_eq!(second.chunks[0].data, b"bbbb");
        assert_eq!(second.next_seq, 3);

        let third = ring.read_since(second.next_seq, 4);
        assert_eq!(third.chunks[0].data, b"cccc");
        assert_eq!(third.next_seq, 4);
        assert!(ring.read_since(third.next_seq, 4).is_empty());
    }

    #[test]
    fn oversized_chunk_still_advances_the_cursor() {
        let ring = RingBuffer::new(4096);
        ring.push(Direction::Rx, b"0123456789");
        let batch = ring.read_since(1, 4);
        assert_eq!(batch.chunks.len(), 1);
        assert_eq!(batch.next_seq, 2);
        assert!(ring.read_since(2, 4).is_empty());
    }
}
