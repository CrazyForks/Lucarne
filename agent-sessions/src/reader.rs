use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const DEFAULT_REVERSE_READ_CHUNK_BYTES: usize = 8 * 1024;

/// File-backed session reader boundary for streaming and reverse reads.
pub struct SessionReader {
    file: std::fs::File,
}

impl SessionReader {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            file: std::fs::File::open(path)?,
        })
    }

    pub fn reverse_lines(self) -> std::io::Result<ReverseLines<std::fs::File>> {
        ReverseLines::new(self.file)
    }

    pub fn reverse_lines_limited(
        self,
        max_bytes: u64,
    ) -> std::io::Result<ReverseLines<std::fs::File>> {
        ReverseLines::new_limited(self.file, max_bytes)
    }

    pub fn reverse_lines_before(
        self,
        before_offset: u64,
    ) -> std::io::Result<ReverseLines<std::fs::File>> {
        ReverseLines::new_before(self.file, before_offset)
    }

    pub fn reverse_lines_before_limited(
        self,
        before_offset: u64,
        max_bytes: u64,
    ) -> std::io::Result<ReverseLines<std::fs::File>> {
        ReverseLines::new_before_limited(self.file, before_offset, max_bytes)
    }
}

/// Reads complete lines from the end of a seekable reader.
///
/// Empty and whitespace-only lines are skipped. Returned lines do not include
/// the trailing newline byte.
pub struct ReverseLines<R> {
    reader: R,
    pos: u64,
    min_pos: u64,
    chunk_bytes: usize,
    remnant: Vec<u8>,
    ready: Vec<ReverseLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseLine {
    pub start: u64,
    pub bytes: Vec<u8>,
}

impl<R> ReverseLines<R>
where
    R: Read + Seek,
{
    pub fn new(reader: R) -> std::io::Result<Self> {
        Self::with_chunk_bytes(reader, DEFAULT_REVERSE_READ_CHUNK_BYTES)
    }

    pub fn new_limited(reader: R, max_bytes: u64) -> std::io::Result<Self> {
        Self::with_chunk_bytes_before(
            reader,
            DEFAULT_REVERSE_READ_CHUNK_BYTES,
            None,
            Some(max_bytes),
        )
    }

    pub fn new_before(reader: R, before_offset: u64) -> std::io::Result<Self> {
        Self::with_chunk_bytes_before(
            reader,
            DEFAULT_REVERSE_READ_CHUNK_BYTES,
            Some(before_offset),
            None,
        )
    }

    pub fn new_before_limited(
        reader: R,
        before_offset: u64,
        max_bytes: u64,
    ) -> std::io::Result<Self> {
        Self::with_chunk_bytes_before(
            reader,
            DEFAULT_REVERSE_READ_CHUNK_BYTES,
            Some(before_offset),
            Some(max_bytes),
        )
    }

    fn with_chunk_bytes(reader: R, chunk_bytes: usize) -> std::io::Result<Self> {
        Self::with_chunk_bytes_before(reader, chunk_bytes, None, None)
    }

    fn with_chunk_bytes_before(
        mut reader: R,
        chunk_bytes: usize,
        before_offset: Option<u64>,
        max_bytes: Option<u64>,
    ) -> std::io::Result<Self> {
        let end = reader.seek(SeekFrom::End(0))?;
        let pos = before_offset.map(|offset| offset.min(end)).unwrap_or(end);
        let min_pos = max_bytes
            .map(|max_bytes| pos.saturating_sub(max_bytes))
            .unwrap_or(0);
        Ok(Self {
            reader,
            pos,
            min_pos,
            chunk_bytes: chunk_bytes.max(1),
            remnant: Vec::new(),
            ready: Vec::new(),
        })
    }

    pub fn lower_bound(&self) -> u64 {
        self.min_pos
    }

    pub fn next_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.next_line_with_start()?.map(|line| line.bytes))
    }

    pub fn next_line_with_start(&mut self) -> std::io::Result<Option<ReverseLine>> {
        loop {
            while let Some(line) = self.ready.pop() {
                if !line.bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
                    return Ok(Some(line));
                }
            }

            if self.pos == self.min_pos {
                if self.min_pos > 0 {
                    return Ok(None);
                }
                if self.remnant.is_empty() {
                    return Ok(None);
                }
                let line = std::mem::take(&mut self.remnant);
                if line.iter().all(|byte| byte.is_ascii_whitespace()) {
                    return Ok(None);
                }
                return Ok(Some(ReverseLine {
                    start: 0,
                    bytes: line,
                }));
            }

            let available = self.pos.saturating_sub(self.min_pos);
            let chunk_len = available.min(self.chunk_bytes as u64) as usize;
            self.pos -= chunk_len as u64;
            let chunk_start = self.pos;
            self.reader.seek(SeekFrom::Start(self.pos))?;
            let mut combined = vec![0; chunk_len];
            self.reader.read_exact(&mut combined)?;
            combined.extend_from_slice(&self.remnant);

            let parts = combined
                .split(|byte| *byte == b'\n')
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>();
            let mut offset = chunk_start;
            let mut positioned = Vec::with_capacity(parts.len());
            for part in parts {
                let start = offset;
                offset = offset.saturating_add(part.len() as u64).saturating_add(1);
                positioned.push(ReverseLine { start, bytes: part });
            }
            self.remnant = positioned.remove(0).bytes;
            self.ready = positioned;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, SeekFrom};

    use super::ReverseLines;

    struct PoisonPrefixReader {
        inner: Cursor<Vec<u8>>,
        poison_until: u64,
    }

    impl PoisonPrefixReader {
        fn new(bytes: &[u8], poison_until: u64) -> Self {
            Self {
                inner: Cursor::new(bytes.to_vec()),
                poison_until,
            }
        }
    }

    impl Read for PoisonPrefixReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let pos = self.inner.position();
            if pos < self.poison_until {
                return Err(std::io::Error::other("read entered poison prefix"));
            }
            self.inner.read(buf)
        }
    }

    impl Seek for PoisonPrefixReader {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    fn collect(input: &[u8], chunk_bytes: usize) -> Vec<String> {
        let mut reader = ReverseLines::with_chunk_bytes(Cursor::new(input), chunk_bytes).unwrap();
        let mut lines = Vec::new();
        while let Some(line) = reader.next_line().unwrap() {
            lines.push(String::from_utf8(line).unwrap());
        }
        lines
    }

    #[test]
    fn reverse_lines_returns_none_for_empty_or_whitespace_only_input() {
        assert!(collect(b"", 1).is_empty());
        assert!(collect(b"\n \n\t\n", 1).is_empty());
    }

    #[test]
    fn reverse_lines_handles_trailing_newline_and_skips_blank_lines() {
        assert_eq!(collect(b"first\nsecond\n\n", 4), vec!["second", "first"]);
        assert_eq!(collect(b"first\nsecond", 4), vec!["second", "first"]);
    }

    #[test]
    fn reverse_lines_stitches_lines_across_chunk_boundaries() {
        assert_eq!(
            collect(b"alpha\nbeta beta\ngamma gamma gamma", 1),
            vec!["gamma gamma gamma", "beta beta", "alpha"]
        );
        assert_eq!(
            collect(b"alpha\nbeta beta\ngamma gamma gamma", 7),
            vec!["gamma gamma gamma", "beta beta", "alpha"]
        );
    }

    #[test]
    fn reverse_lines_stops_before_older_chunks_when_caller_stops() {
        let bytes = b"poison-one\npoison-two\nsafe-one\nsafe-two\n";
        let poison_until = b"poison-one\npoison-two".len() as u64;
        let mut reader =
            ReverseLines::with_chunk_bytes(PoisonPrefixReader::new(bytes, poison_until), 1)
                .unwrap();

        assert_eq!(
            reader.next_line().unwrap().as_deref(),
            Some(&b"safe-two"[..])
        );
        assert_eq!(
            reader.next_line().unwrap().as_deref(),
            Some(&b"safe-one"[..])
        );
    }
}
