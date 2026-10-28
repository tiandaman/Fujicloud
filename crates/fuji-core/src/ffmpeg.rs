//! FFmpeg, for movies Windows can't decode itself: iPhone HEVC on a PC
//! without Microsoft's HEVC Video Extensions (which cost money, and which
//! many PCs don't have). Downloaded once, the first time it's needed, and
//! run as a separate program: movies are converted on this PC, never sent
//! anywhere.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::Error;
use crate::decode::Rgb;

/// FFmpeg 6.1.1 for 64-bit Windows (gyan.dev's build, as packaged by
/// ffmpeg-static), one gzipped program.
#[cfg(feature = "smart-search")]
const URL: &str =
    "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-win32-x64.gz";
/// The download, so a changed or damaged file is never run.
#[cfg(feature = "smart-search")]
const SHA256: &str = "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77";
pub const DOWNLOAD_MB: u32 = 30;
const EXE: &str = "ffmpeg-6.1.1.exe";

fn bad(msg: impl Into<String>) -> Error {
    Error::Decode(msg.into())
}

/// Where the models go; FFmpeg is kept next to them.
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Tells posters where a downloaded FFmpeg would be.
pub fn set_dir(dir: &Path) {
    let _ = DIR.set(dir.to_owned());
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("ffmpeg").join(EXE)
}

/// FFmpeg, if it has been downloaded (or, for tests, `FUJICLOUD_FFMPEG`).
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FUJICLOUD_FFMPEG") {
        return Some(p.into());
    }
    DIR.get().map(|d| path(d)).filter(|p| p.is_file())
}

/// Fetches FFmpeg (~30 MB) and checks it. `progress(done, total)` in bytes.
#[cfg(feature = "smart-search")]
pub fn download(dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, Error> {
    use sha2::{Digest, Sha256};
    let exe = path(dir);
    if exe.is_file() {
        return Ok(exe);
    }
    let gz = dir.join("ffmpeg").join("ffmpeg-6.1.1.gz");
    crate::embed::download_files(&[(URL, gz.clone())], progress)?;
    let packed = std::fs::read(&gz)?;
    let sum: String = Sha256::digest(&packed)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if sum != SHA256 {
        let _ = std::fs::remove_file(&gz);
        return Err(Error::Model(format!(
            "the movie converter that was downloaded isn't the right file ({sum})"
        )));
    }
    let mut program = Vec::new();
    flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut program)?;
    let part = exe.with_extension("part");
    std::fs::write(&part, program)?;
    std::fs::rename(&part, &exe)?;
    let _ = std::fs::remove_file(&gz);
    Ok(exe)
}

fn command(ffmpeg: &Path) -> Command {
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-nostdin"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// What FFmpeg says about a movie: its length, and whether it's HDR.
#[derive(Debug, Default, PartialEq)]
struct Probe {
    seconds: f64,
    hdr: bool,
}

fn parse_probe(text: &str) -> Probe {
    let seconds = text
        .split("Duration: ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|t| {
            let mut parts = t.trim().split(':').map(|p| p.parse::<f64>().ok());
            Some(parts.next()?? * 3600.0 + parts.next()?? * 60.0 + parts.next()??)
        })
        .unwrap_or(0.0);
    let hdr = text
        .lines()
        .filter(|l| l.contains("Video:"))
        .take(1)
        .any(|l| l.contains("arib-std-b67") || l.contains("smpte2084"));
    Probe { seconds, hdr }
}

fn probe(ffmpeg: &Path, src: &Path) -> Result<Probe, Error> {
    // With no output FFmpeg describes the input, then "fails".
    let out = command(ffmpeg).arg("-i").arg(src).output()?;
    let text = String::from_utf8_lossy(&out.stderr);
    if !text.contains("Video:") {
        return Err(bad(format!(
            "FFmpeg can't read the movie: {}",
            last_line(&text)
        )));
    }
    Ok(parse_probe(&text))
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// iPhone HDR (HLG) brought down to an ordinary picture, as the window
/// shows SDR; without it the colours come out washed out.
const TONEMAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv";

/// Re-encodes a movie as H.264/AAC MP4, which the window plays. Turned
/// upright, HDR tone-mapped. `progress` gets 0–1.
pub fn transcode(
    ffmpeg: &Path,
    src: &Path,
    dest: &Path,
    progress: &dyn Fn(f32),
) -> Result<(), Error> {
    let info = probe(ffmpeg, src)?;
    let mut filters = Vec::new();
    if info.hdr {
        filters.push(TONEMAP);
    }
    // Even sizes, which H.264 needs.
    filters.push("scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p");
    let mut child = command(ffmpeg)
        .arg("-i")
        .arg(src)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf"])
        .arg(filters.join(","))
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-c:a",
            "aac",
            "-b:a",
            "160k",
            "-ac",
            "2",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
            "-progress",
            "pipe:1",
            "-nostats",
            "-loglevel",
            "error",
            "-y",
        ])
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    if let Some(out) = child.stdout.take() {
        use std::io::BufRead;
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Some(us) = line.strip_prefix("out_time_us=")
                && let Ok(us) = us.trim().parse::<f64>()
                && info.seconds > 0.0
            {
                progress((us / 1e6 / info.seconds).clamp(0.0, 0.99) as f32);
            }
        }
    }
    let status = child.wait()?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        return Err(bad(format!(
            "FFmpeg couldn't convert the movie: {}",
            last_line(&errors)
        )));
    }
    progress(1.0);
    Ok(())
}

/// A frame about a second in (first frames are often black), upright,
/// fitting in `long_edge`.
pub fn frame(ffmpeg: &Path, src: &Path, long_edge: u32) -> Result<Rgb, Error> {
    let info = probe(ffmpeg, src)?;
    let at = if info.seconds > 2.0 { 1.0 } else { 0.0 };
    let mut filters = Vec::new();
    if info.hdr {
        filters.push(TONEMAP.to_owned());
    }
    filters.push(format!(
        "scale=w={long_edge}:h={long_edge}:force_original_aspect_ratio=decrease,format=rgb24"
    ));
    let out = command(ffmpeg)
        .args(["-ss", &format!("{at}")])
        .arg("-i")
        .arg(src)
        .args(["-frames:v", "1", "-vf"])
        .arg(filters.join(","))
        .args([
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "-loglevel",
            "error",
            "pipe:1",
        ])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(bad(format!(
            "FFmpeg couldn't take a frame: {}",
            last_line(&String::from_utf8_lossy(&out.stderr))
        )));
    }
    let decoder = png::Decoder::new(std::io::Cursor::new(out.stdout));
    let mut reader = decoder.read_info().map_err(|e| bad(e.to_string()))?;
    let mut data = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut data)
        .map_err(|e| bad(e.to_string()))?;
    if info.color_type != png::ColorType::Rgb || info.bit_depth != png::BitDepth::Eight {
        return Err(bad("FFmpeg's frame isn't 8-bit RGB"));
    }
    data.truncate(info.buffer_size());
    Ok(Rgb {
        data,
        width: info.width,
        height: info.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_ffmpeg_says() {
        let text = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'IMG_6584.MOV':
  Duration: 00:01:09.53, start: 0.000000, bitrate: 9140 kb/s
  Stream #0:0[0x1](und): Video: hevc (Main 10) (hvc1 / 0x31637668), yuv420p10le(tv, bt2020nc/bt2020/arib-std-b67), 1920x1080, 8960 kb/s, 29.98 fps
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 160 kb/s";
        assert_eq!(
            parse_probe(text),
            Probe {
                seconds: 69.53,
                hdr: true
            }
        );
        let sdr = "  Duration: 00:00:02.00, start: 0.000000\n  Stream #0:0: Video: hevc (Main), yuv420p(tv, bt709), 640x360";
        assert_eq!(
            parse_probe(sdr),
            Probe {
                seconds: 2.0,
                hdr: false
            }
        );
    }

    /// With a real FFmpeg (`FUJICLOUD_FFMPEG`, or the one downloaded into
    /// `FUJICLOUD_MODEL_DIR`): an HEVC clip, SDR and HDR, comes out as
    /// H.264 the window can play, and gives a poster.
    #[test]
    #[ignore]
    fn real_ffmpeg() {
        let ffmpeg = match std::env::var_os("FUJICLOUD_FFMPEG") {
            Some(p) => PathBuf::from(p),
            None => {
                #[cfg(feature = "smart-search")]
                {
                    let dir = PathBuf::from(std::env::var("FUJICLOUD_MODEL_DIR").unwrap());
                    download(&dir, &|_, _| {}).unwrap()
                }
                #[cfg(not(feature = "smart-search"))]
                panic!("set FUJICLOUD_FFMPEG")
            }
        };
        let dir = std::env::temp_dir().join(format!("fuji-ffmpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, extra) in [
            ("sdr.mov", vec![]),
            (
                "hlg.mov",
                vec![
                    "-pix_fmt",
                    "yuv420p10le",
                    "-color_primaries",
                    "bt2020",
                    "-color_trc",
                    "arib-std-b67",
                    "-colorspace",
                    "bt2020nc",
                ],
            ),
        ] {
            let clip = dir.join(name);
            // An iPhone-like clip: HEVC tagged hvc1, with sound.
            let made = Command::new(&ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=640x360:rate=30:duration=3",
                ])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
                .args([
                    "-c:v",
                    "libx265",
                    "-tag:v",
                    "hvc1",
                    "-x265-params",
                    "log-level=error",
                ])
                .args(&extra)
                .args(["-c:a", "aac", "-shortest"])
                .arg(&clip)
                .status()
                .unwrap();
            assert!(made.success(), "making {name}");
            let info = probe(&ffmpeg, &clip).unwrap();
            assert_eq!(info.hdr, name == "hlg.mov", "{name}: {info:?}");
            assert!((info.seconds - 3.0).abs() < 0.2, "{name}: {info:?}");

            let out = dir.join(format!("{name}.mp4"));
            let seen = std::sync::Mutex::new(Vec::new());
            transcode(&ffmpeg, &clip, &out, &|p| seen.lock().unwrap().push(p)).unwrap();
            let seen = seen.into_inner().unwrap();
            assert_eq!(seen.last(), Some(&1.0), "{name}: {seen:?}");
            let text = String::from_utf8_lossy(
                &command(&ffmpeg)
                    .arg("-i")
                    .arg(&out)
                    .output()
                    .unwrap()
                    .stderr,
            )
            .into_owned();
            assert!(text.contains("Video: h264"), "{name}: {text}");
            assert!(
                text.contains("yuv420p(") && !text.contains("arib"),
                "{name}: {text}"
            );
            assert!(text.contains("Audio: aac"), "{name}: {text}");
            assert!(text.contains("640x360"), "{name}: {text}");
            // And Windows itself can read the result.
            #[cfg(windows)]
            {
                let f = crate::video::decoded_frame(&out).unwrap();
                assert_eq!((f.width, f.height), (640, 360), "{name}");
            }

            let poster = frame(&ffmpeg, &clip, 320).unwrap();
            assert_eq!((poster.width, poster.height), (320, 180), "{name}");
            assert_eq!(poster.data.len(), 320 * 180 * 3);
            // testsrc2 is colourful, not a black frame.
            let mean =
                poster.data.iter().map(|&b| b as u64).sum::<u64>() / poster.data.len() as u64;
            assert!(mean > 40, "{name}: mean {mean}");
            eprintln!("{name}: converted, poster mean {mean}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
