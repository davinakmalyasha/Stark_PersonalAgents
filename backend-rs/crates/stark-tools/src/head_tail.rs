use std::collections::VecDeque;

pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct HeadTailBuffer {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted_bytes: usize,
}

impl HeadTailBuffer {
    const HEAD_BUDGET: usize = MAX_OUTPUT_BYTES / 2;
    const TAIL_BUDGET: usize = MAX_OUTPUT_BYTES - Self::HEAD_BUDGET;

    pub fn push_chunk(&mut self, chunk: &[u8]) {
        let remaining_head = Self::HEAD_BUDGET.saturating_sub(self.head.len());
        let (head_chunk, tail_chunk) = chunk
            .split_at_checked(remaining_head)
            .unwrap_or((chunk, &[]));
        self.head.extend_from_slice(head_chunk);
        self.push_tail(tail_chunk);
    }

    fn push_tail(&mut self, chunk: &[u8]) {
        let remaining_tail = Self::TAIL_BUDGET.saturating_sub(self.tail.len());
        let excess_tail = chunk.len().saturating_sub(remaining_tail);
        self.omitted_bytes = self.omitted_bytes.saturating_add(excess_tail);

        let chunk = match excess_tail.checked_sub(self.tail.len()) {
            None => {
                self.tail.drain(..excess_tail);
                chunk
            }
            Some(skip) => {
                self.tail.clear();
                &chunk[skip..]
            }
        };
        self.tail.extend(chunk);
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len());
        out.extend_from_slice(&self.head);
        out.extend(self.tail.iter().copied());
        out
    }

    pub fn to_bytes_with_omission_marker(&self) -> Vec<u8> {
        if self.omitted_bytes == 0 {
            return self.to_bytes();
        }
        let marker = format!("... {} bytes omitted ...", self.omitted_bytes);
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len() + marker.len() + 2);
        out.extend_from_slice(&self.head);
        out.push(b'\n');
        out.extend_from_slice(marker.as_bytes());
        out.push(b'\n');
        out.extend(self.tail.iter().copied());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_head_and_tail() {
        let mut buffer = HeadTailBuffer::default();
        buffer.push_chunk(b"head");
        buffer.push_chunk(&vec![b'x'; MAX_OUTPUT_BYTES]);
        buffer.push_chunk(b"tail");
        let output = buffer.to_bytes_with_omission_marker();
        let text = String::from_utf8_lossy(&output);
        assert!(text.starts_with("head"));
        assert!(text.ends_with("tail"));
        assert!(text.contains("bytes omitted"));
    }
}
