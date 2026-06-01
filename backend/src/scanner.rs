use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use crate::search::ContextLine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub path: PathBuf,
    pub line_no: u64,
    pub offset: u64,
    pub content: String,
}

pub fn scan_lines(path: &Path) -> anyhow::Result<Vec<LogLine>> {
    let mut lines = Vec::new();
    scan_lines_from(path, 0, 1, |line| {
        lines.push(line);
        Ok(())
    })?;
    Ok(lines)
}

pub fn scan_lines_from<F>(
    path: &Path,
    start_offset: u64,
    start_line_no: u64,
    mut on_line: F,
) -> anyhow::Result<(u64, u64)>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(start_offset))?;
    let mut offset = start_offset;
    let mut line_no = start_line_no;

    loop {
        let start = offset;
        let mut buf = String::new();
        let bytes = reader.read_line(&mut buf)?;
        if bytes == 0 {
            break;
        }

        offset += bytes as u64;
        while buf.ends_with('\n') || buf.ends_with('\r') {
            buf.pop();
        }

        on_line(LogLine {
            path: path.to_path_buf(),
            line_no,
            offset: start,
            content: buf,
        })?;

        line_no += 1;
    }

    Ok((offset, line_no.saturating_sub(1)))
}

pub fn scan_reader_lines<F, R>(
    path: &Path,
    reader: R,
    start_line_no: u64,
    mut on_line: F,
) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
    R: Read,
{
    let reader = BufReader::new(reader);
    let mut line_no = start_line_no;
    let mut logical_offset = 0_u64;

    for line in reader.lines() {
        let content = line?;
        on_line(LogLine {
            path: path.to_path_buf(),
            line_no,
            offset: logical_offset,
            content,
        })?;
        line_no += 1;
        logical_offset += 1;
    }

    Ok(line_no.saturating_sub(1))
}

pub fn scan_gzip_lines<F>(path: &Path, on_line: F) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    scan_reader_lines(path, decoder, 1, on_line)
}

pub fn scan_zstd_lines<F>(path: &Path, on_line: F) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    scan_reader_lines(path, decoder, 1, on_line)
}

pub fn scan_bzip2_lines<F>(path: &Path, on_line: F) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    scan_reader_lines(path, decoder, 1, on_line)
}

pub fn scan_xz_lines<F>(path: &Path, on_line: F) -> anyhow::Result<u64>
where
    F: FnMut(LogLine) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let decoder = xz2::read::XzDecoder::new(file);
    scan_reader_lines(path, decoder, 1, on_line)
}

pub fn read_context(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    read_context_from_reader(File::open(path)?, line_no, before, after)
}

pub fn read_gzip_context(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let file = File::open(path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    read_context_from_reader(decoder, line_no, before, after)
}

pub fn read_context_lines(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    read_context_lines_from_reader(File::open(path)?, line_no, before, after)
}

pub fn read_gzip_context_lines(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    let file = File::open(path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    read_context_lines_from_reader(decoder, line_no, before, after)
}

pub fn read_zstd_context_lines(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    read_context_lines_from_reader(decoder, line_no, before, after)
}

pub fn read_bzip2_context_lines(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    let file = File::open(path)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    read_context_lines_from_reader(decoder, line_no, before, after)
}

pub fn read_xz_context_lines(
    path: &Path,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    let file = File::open(path)?;
    let decoder = xz2::read::XzDecoder::new(file);
    read_context_lines_from_reader(decoder, line_no, before, after)
}

fn read_context_from_reader<R: Read>(
    reader: R,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let reader = BufReader::new(reader);
    let before_start = line_no.saturating_sub(before as u64).max(1);
    let after_end = line_no.saturating_add(after as u64);
    let mut before_lines = Vec::new();
    let mut after_lines = Vec::new();

    for (idx, line) in reader.lines().enumerate() {
        let current = idx as u64 + 1;
        if current < before_start {
            continue;
        }
        if current > after_end {
            break;
        }

        let line = line?;
        if current < line_no {
            before_lines.push(line);
        } else if current > line_no {
            after_lines.push(line);
        }
    }

    Ok((before_lines, after_lines))
}

fn read_context_lines_from_reader<R: Read>(
    reader: R,
    line_no: u64,
    before: usize,
    after: usize,
) -> anyhow::Result<Vec<ContextLine>> {
    let reader = BufReader::new(reader);
    let before_start = line_no.saturating_sub(before as u64).max(1);
    let after_end = line_no.saturating_add(after as u64);
    let mut lines = Vec::with_capacity(before + after + 1);
    let mut logical_offset = 0_u64;

    for (idx, line) in reader.lines().enumerate() {
        let current = idx as u64 + 1;
        let content = line?;
        let line_offset = logical_offset;
        logical_offset += content.len() as u64 + 1;

        if current < before_start {
            continue;
        }
        if current > after_end {
            break;
        }

        lines.push(ContextLine {
            line_no: current,
            offset: line_offset,
            content,
        });
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn scanner_records_line_numbers_and_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut file = File::create(&path).unwrap();
        write!(file, "first\nsecond\nthird").unwrap();

        let lines = scan_lines(&path).unwrap();

        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[0].offset, 0);
        assert_eq!(lines[1].line_no, 2);
        assert_eq!(lines[1].offset, 6);
        assert_eq!(lines[2].line_no, 3);
        assert_eq!(lines[2].content, "third");
    }

    #[test]
    fn scanner_can_resume_from_offset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut file = File::create(&path).unwrap();
        write!(file, "first\nsecond\nthird\n").unwrap();

        let mut lines = Vec::new();
        let (offset, line_no) = scan_lines_from(&path, 6, 2, |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();

        assert_eq!(line_no, 3);
        assert_eq!(offset, 19);
        assert_eq!(lines[0].line_no, 2);
        assert_eq!(lines[0].content, "second");
    }

    #[test]
    fn context_reads_lines_around_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut file = File::create(&path).unwrap();
        write!(file, "one\ntwo\nthree\nfour\nfive\n").unwrap();

        let (before, after) = read_context(&path, 3, 2, 1).unwrap();

        assert_eq!(before, vec!["one", "two"]);
        assert_eq!(after, vec!["four"]);
    }

    #[test]
    fn context_lines_include_real_line_numbers_and_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let mut file = File::create(&path).unwrap();
        write!(file, "one\ntwo\nthree\nfour\nfive\n").unwrap();

        let context = read_context_lines(&path, 3, 1, 1).unwrap();

        assert_eq!(context.len(), 3);
        assert_eq!(context[0].line_no, 2);
        assert_eq!(context[0].offset, 4);
        assert_eq!(context[0].content, "two");
        assert_eq!(context[1].line_no, 3);
        assert_eq!(context[1].offset, 8);
        assert_eq!(context[2].line_no, 4);
    }

    #[test]
    fn gzip_scanner_reads_compressed_lines() {
        use flate2::{Compression, write::GzEncoder};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log.1.gz");
        let file = File::create(&path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        write!(encoder, "one\ntwo timeout\nthree\n").unwrap();
        encoder.finish().unwrap();

        let mut lines = Vec::new();
        let last_line = scan_gzip_lines(&path, |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();
        let (before, after) = read_gzip_context(&path, 2, 1, 1).unwrap();

        assert_eq!(last_line, 3);
        assert_eq!(lines[1].content, "two timeout");
        assert_eq!(before, vec!["one"]);
        assert_eq!(after, vec!["three"]);
    }

    #[test]
    fn zstd_scanner_reads_compressed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log.1.zst");
        let compressed = zstd::encode_all("one\ntwo timeout\nthree\n".as_bytes(), 0).unwrap();
        std::fs::write(&path, compressed).unwrap();

        let mut lines = Vec::new();
        let last_line = scan_zstd_lines(&path, |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();
        let context = read_zstd_context_lines(&path, 2, 1, 1).unwrap();

        assert_eq!(last_line, 3);
        assert_eq!(lines[1].content, "two timeout");
        assert_eq!(context.len(), 3);
        assert_eq!(context[1].content, "two timeout");
    }

    #[test]
    fn bzip2_scanner_reads_compressed_lines() {
        use bzip2::{Compression, write::BzEncoder};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log.1.bz2");
        let file = File::create(&path).unwrap();
        let mut encoder = BzEncoder::new(file, Compression::default());
        write!(encoder, "one\ntwo timeout\nthree\n").unwrap();
        encoder.finish().unwrap();

        let mut lines = Vec::new();
        let last_line = scan_bzip2_lines(&path, |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();
        let context = read_bzip2_context_lines(&path, 2, 1, 1).unwrap();

        assert_eq!(last_line, 3);
        assert_eq!(lines[1].content, "two timeout");
        assert_eq!(context.len(), 3);
        assert_eq!(context[1].content, "two timeout");
    }

    #[test]
    fn xz_scanner_reads_compressed_lines() {
        use xz2::write::XzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log.1.xz");
        let file = File::create(&path).unwrap();
        let mut encoder = XzEncoder::new(file, 6);
        write!(encoder, "one\ntwo timeout\nthree\n").unwrap();
        encoder.finish().unwrap();

        let mut lines = Vec::new();
        let last_line = scan_xz_lines(&path, |line| {
            lines.push(line);
            Ok(())
        })
        .unwrap();
        let context = read_xz_context_lines(&path, 2, 1, 1).unwrap();

        assert_eq!(last_line, 3);
        assert_eq!(lines[1].content, "two timeout");
        assert_eq!(context.len(), 3);
        assert_eq!(context[1].content, "two timeout");
    }
}
