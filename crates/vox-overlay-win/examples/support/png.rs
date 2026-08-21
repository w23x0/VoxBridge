//! 零依赖 PNG 写出器：只用 std，供 examples 落盘截图 / 调试图用。
//!
//! 之所以不拉 `png` / `image`：这些代码只在示例里一次性落盘，为它给 crate
//! 增加编译期依赖不划算。压缩率同样不重要，所以直接用 deflate 的 "stored"
//! （不压缩）块——它照样是完全合法的 zlib 流，于是整个压缩器都能省掉。

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// PNG 固定 8 字节签名（RFC 2083 §3.1）。
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// 单个 stored 块的载荷上限：块长度字段只有 u16，再多就装不下。
const MAX_STORED: usize = u16::MAX as usize;

/// 把 8 位 RGBA 像素写成 PNG 文件。`rgba.len()` 必须等于 `w * h * 4`。
pub fn write_rgba(path: &Path, w: u32, h: u32, rgba: &[u8]) -> io::Result<()> {
    // 颜色类型 6 = 真彩色带 alpha
    write_png(path, w, h, rgba, 4, 6)
}

/// 把 8 位灰度像素写成 PNG 文件。`gray.len()` 必须等于 `w * h`。
pub fn write_gray(path: &Path, w: u32, h: u32, gray: &[u8]) -> io::Result<()> {
    // 颜色类型 0 = 单通道灰度
    write_png(path, w, h, gray, 1, 0)
}

/// RGBA / 灰度共用的落盘主体：`channels` 与 `color_type` 必须自洽。
fn write_png(
    path: &Path,
    w: u32,
    h: u32,
    pixels: &[u8],
    channels: usize,
    color_type: u8,
) -> io::Result<()> {
    if w == 0 || h == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("图像尺寸不能为 0（w={w}, h={h}）"),
        ));
    }

    // 用 checked 乘法而非直接相乘：32 位 target 上大尺寸图会静默回绕，
    // 那会让长度校验形同虚设。
    let expected = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(channels))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("图像尺寸溢出 usize（w={w}, h={h}, channels={channels}）"),
            )
        })?;
    if pixels.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "像素缓冲长度 {} 与 {w}x{h}x{channels}={expected} 不符",
                pixels.len()
            ),
        ));
    }

    // 组装 raw 扫描线：PNG 要求每行前面多一个 filter 字节，这里统一用 0（None），
    // 因为不追求压缩率，滤波器毫无收益。
    let stride = (w as usize) * channels;
    let mut raw = Vec::with_capacity(expected + h as usize);
    for row in pixels.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(color_type);
    ihdr.push(0); // compression: deflate，唯一合法值
    ihdr.push(0); // filter method 0
    ihdr.push(0); // 非隔行

    let mut out = BufWriter::new(File::create(path)?);
    out.write_all(&SIGNATURE)?;
    write_chunk(&mut out, b"IHDR", &ihdr)?;
    write_chunk(&mut out, b"IDAT", &zlib_stored(&raw))?;
    write_chunk(&mut out, b"IEND", &[])?;
    // 显式 flush：BufWriter 在 drop 里丢弃错误，落盘失败必须能被调用方看到。
    out.flush()
}

/// 写一个 PNG chunk：大端长度 + 4 字节类型 + 数据 + 覆盖「类型+数据」的大端 CRC32。
fn write_chunk<W: Write>(out: &mut W, kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    let len = u32::try_from(data.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("chunk {} 超过 u32 长度上限", String::from_utf8_lossy(kind)),
        )
    })?;
    out.write_all(&len.to_be_bytes())?;
    out.write_all(kind)?;
    out.write_all(data)?;

    // CRC 的覆盖范围含类型码但不含长度字段，所以分两步喂进去。
    let crc = crc32_update(crc32_update(0xffff_ffff, kind), data) ^ 0xffff_ffff;
    out.write_all(&crc.to_be_bytes())
}

/// 把 `raw` 包成合法 zlib 流（RFC 1950 外壳 + RFC 1951 stored 块）。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    // 每块最多 65535 字节载荷、5 字节头，再加 2 字节 zlib 头与 4 字节 Adler。
    let blocks = raw.len() / MAX_STORED + 1;
    let mut out = Vec::with_capacity(raw.len() + blocks * 5 + 6);

    // 0x78 0x01：CM=8/CINFO=7（32K 窗口），FLEVEL=0，且 0x7801 % 31 == 0 满足校验。
    out.extend_from_slice(&[0x78, 0x01]);

    if raw.is_empty() {
        // 空数据也必须有一个 BFINAL 块，否则解码器会一直等后续块。
        out.push(1);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        // peekable 判定「是否还有下一块」：这样 raw.len() 正好是 65535 的整数倍时，
        // BFINAL 也会落在最后一个真实块上，而不会多出一个空块或提前收尾。
        let mut chunks = raw.chunks(MAX_STORED).peekable();
        while let Some(block) = chunks.next() {
            let final_block = chunks.peek().is_none();
            // bit0 = BFINAL，bit1..2 = BTYPE 00（stored）；其余位在字节对齐后无意义。
            out.push(u8::from(final_block));
            // chunks 保证 len <= MAX_STORED，故此处转换不会截断。
            let len = block.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            // NLEN 规定为 LEN 的按位取反，供解码器做一次廉价的自检。
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
        }
    }

    // Adler-32 校验的是「未压缩」数据，与块是否压缩无关，所以直接喂 raw。
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// 增量 CRC-32（反射多项式 0xEDB88320），逐位算以免维护 256 项静态表。
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            // 反射实现里最低位才是待移出的位，因此判低位、向右移。
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// Adler-32：高半部是 b（a 的前缀和），低半部是 a（字节和），模 65521。
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    // 5552 是使 b 不会溢出 u32 的最大分段长度（zlib 的 NMAX），
    // 按段取模比每字节取模快得多。
    const NMAX: usize = 5552;

    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for segment in data.chunks(NMAX) {
        for &byte in segment {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}
