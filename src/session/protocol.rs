use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const FRAME_STDIN: u8 = 0x01;
pub const FRAME_STDOUT: u8 = 0x02;
pub const FRAME_RESIZE: u8 = 0x03;
pub const FRAME_EXIT: u8 = 0x04;

const MAX_FRAME: u32 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Stdin(Vec<u8>),
    Stdout(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Exit { code: i32 },
}

impl Frame {
    pub fn kind(&self) -> u8 {
        match self {
            Frame::Stdin(_) => FRAME_STDIN,
            Frame::Stdout(_) => FRAME_STDOUT,
            Frame::Resize { .. } => FRAME_RESIZE,
            Frame::Exit { .. } => FRAME_EXIT,
        }
    }

    pub fn payload(&self) -> Vec<u8> {
        match self {
            Frame::Stdin(b) | Frame::Stdout(b) => b.clone(),
            Frame::Resize { cols, rows } => {
                let mut out = Vec::with_capacity(4);
                out.extend_from_slice(&cols.to_be_bytes());
                out.extend_from_slice(&rows.to_be_bytes());
                out
            }
            Frame::Exit { code } => code.to_be_bytes().to_vec(),
        }
    }
}

pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, frame: &Frame) -> Result<()> {
    let payload = frame.payload();
    if payload.len() as u32 > MAX_FRAME {
        return Err(anyhow!("frame payload too large"));
    }
    w.write_u8(frame.kind()).await?;
    w.write_u32(payload.len() as u32).await?;
    if !payload.is_empty() {
        w.write_all(&payload).await?;
    }
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Option<Frame>> {
    let mut kind_buf = [0u8; 1];
    match r.read_exact(&mut kind_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = r.read_u32().await? as usize;
    if len as u32 > MAX_FRAME {
        return Err(anyhow!("frame payload too large: {len}"));
    }
    let mut buf = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut buf).await?;
    }
    let frame = match kind_buf[0] {
        FRAME_STDIN => Frame::Stdin(buf),
        FRAME_STDOUT => Frame::Stdout(buf),
        FRAME_RESIZE => {
            if buf.len() != 4 {
                return Err(anyhow!("resize frame must be 4 bytes"));
            }
            let cols = u16::from_be_bytes([buf[0], buf[1]]);
            let rows = u16::from_be_bytes([buf[2], buf[3]]);
            Frame::Resize { cols, rows }
        }
        FRAME_EXIT => {
            if buf.len() != 4 {
                return Err(anyhow!("exit frame must be 4 bytes"));
            }
            let code = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
            Frame::Exit { code }
        }
        other => return Err(anyhow!("unknown frame kind: {other:#x}")),
    };
    Ok(Some(frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn round_trip_all_frames() {
        let frames = vec![
            Frame::Stdin(b"hello".to_vec()),
            Frame::Stdout(vec![0, 1, 2, 3, 4]),
            Frame::Resize {
                cols: 120,
                rows: 40,
            },
            Frame::Exit { code: 0 },
            Frame::Exit { code: -1 },
        ];

        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.expect("write");
        }

        let cursor = Cursor::new(buf);
        let mut reader = BufReader::new(cursor);
        for expected in &frames {
            let got = read_frame(&mut reader).await.expect("read").expect("frame");
            assert_eq!(&got, expected);
        }
        let none = read_frame(&mut reader).await.expect("eof");
        assert!(none.is_none());
    }
}
