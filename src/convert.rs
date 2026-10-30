// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Пакетная конвертация: фото (WIC), аудио и видео (Media Foundation — кодеки
//! Windows: H.264, AAC, MP3, WMA, FLAC). Работает в фоновом потоке.

use crate::image_buf::Img;
use crate::kinds::Kind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Что получить из каждого типа файлов.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Фото: индекс в PHOTO_TARGETS.
    pub photo: usize,
    /// Фото: длинная сторона (0 — как есть).
    pub photo_max: u32,
    /// Аудио: индекс в AUDIO_TARGETS.
    pub audio: usize,
    /// Видео: индекс в VIDEO_TARGETS.
    pub video: usize,
    /// Видео: высота кадра (0 — как есть).
    pub video_h: u32,
    /// 0 — высокое, 1 — среднее, 2 — низкое качество.
    pub quality: usize,
}

pub const PHOTO_TARGETS: [(&str, &str); 8] = [
    ("jpg", "JPEG"), ("png", "PNG"), ("bmp", "BMP"), ("tif", "TIFF"), ("gif", "GIF"), ("jxr", "JPEG XR"), ("ico", "ICO"), ("pdf", "PDF"),
];
pub const AUDIO_TARGETS: [(&str, &str); 5] = [("mp3", "MP3"), ("m4a", "AAC"), ("wav", "WAV"), ("flac", "FLAC"), ("wma", "WMA")];
pub const VIDEO_TARGETS: [(&str, &str); 4] = [("mp4", "MP4 (H.264)"), ("wmv", "WMV"), ("mp3", "Только звук MP3"), ("m4a", "Только звук AAC")];

pub enum Msg {
    Progress(usize, f64),
    Done(usize, PathBuf),
    Error(usize, String),
    Finished,
}

/// Расширение результата для файла данного типа.
pub fn target_ext(kind: Kind, s: &Settings) -> &'static str {
    match kind {
        Kind::Image => PHOTO_TARGETS[s.photo].0,
        Kind::Audio => AUDIO_TARGETS[s.audio].0,
        Kind::Video => VIDEO_TARGETS[s.video].0,
        _ => "",
    }
}

/// Имя результата: рядом с исходником или в папке; без перезаписи существующих.
pub fn out_path(src: &Path, ext: &str, dir: Option<&Path>) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "файл".into());
    let dir = dir.map(|d| d.to_path_buf()).or_else(|| src.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut k = 1;
    while p.exists() {
        p = dir.join(format!("{stem} ({k}).{ext}"));
        k += 1;
    }
    p
}

/// Запустить конвертацию в фоне. Сообщения — в канал.
pub fn start(jobs: Vec<(PathBuf, Kind)>, s: Settings, dir: Option<PathBuf>, tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        }
        for (i, (src, kind)) in jobs.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let ext = target_ext(*kind, &s);
            let dst = out_path(src, ext, dir.as_deref());
            let _ = tx.send(Msg::Progress(i, 0.0));
            let progress = |f: f64| {
                let _ = tx.send(Msg::Progress(i, f));
            };
            let r = match kind {
                Kind::Image => photo(src, &dst, &s),
                Kind::Audio | Kind::Video => av(src, &dst, *kind, &s, &progress, &stop),
                _ => Err("этот тип не конвертируется".into()),
            };
            match r {
                Ok(()) => {
                    let _ = tx.send(Msg::Done(i, dst));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&dst);
                    let _ = tx.send(Msg::Error(i, e));
                }
            }
        }
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
        let _ = tx.send(Msg::Finished);
    });
}

// ----------------------------------------------------------------- фото ---
fn photo(src: &Path, dst: &Path, s: &Settings) -> std::result::Result<(), String> {
    let mut img = crate::codec::load(src).map_err(|e| e.message().to_string())?;
    if s.photo_max > 0 && img.w.max(img.h) > s.photo_max {
        img = crate::codec::thumb(&img, s.photo_max);
    }
    let q = [95, 85, 70][s.quality.min(2)];
    if crate::util::ext_lower(dst) == "pdf" {
        return photo_pdf(&img, dst, q);
    }
    crate::codec::save_q(&img, dst, q).map_err(|e| e.message().to_string())
}

fn photo_pdf(img: &Img, dst: &Path, q: i32) -> std::result::Result<(), String> {
    use crate::pdf_write::{photo_layout, write, OutPage, PhotoPage, Placed};
    let jpeg = crate::codec::encode_mem(img, "jpg", q).map_err(|e| e.message().to_string())?;
    let mode = if crate::settings::get_i("pdf_photo", 0) == 1 { PhotoPage::Fit } else { PhotoPage::A4 };
    let (page, place) = photo_layout((img.w, img.h), mode, crate::settings::get_i("pdf_margin", 10) as f64);
    write(&[OutPage::Raster { page, items: vec![Placed { jpeg, px: (img.w, img.h), place, angle: 0.0 }] }], dst)
}

// --------------------------------------------------------- аудио/видео ---
fn err(e: Error) -> String {
    let m = e.message().trim().to_string();
    let code = e.code().0 as u32;
    match code {
        0xC00D36C4 => "формат не поддерживается Windows (нет кодека)".into(),
        0xC00D5212 => "нет подходящего кодировщика в Windows".into(),
        0xC00D36B4 => "кодировщик не принял параметры".into(),
        _ if m.is_empty() => format!("ошибка 0x{code:08X}"),
        _ => m,
    }
}

const ANY: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

fn new_type(major: &GUID, sub: &GUID) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, major)?;
        t.SetGUID(&MF_MT_SUBTYPE, sub)?;
        Ok(t)
    }
}

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// Длительность в 100-нс единицах.
pub fn duration(r: &IMFSourceReader) -> u64 {
    unsafe { r.GetPresentationAttribute(SOURCE, &MF_PD_DURATION).ok().and_then(|v| u64::try_from(&v).ok()).unwrap_or(0) }
}

/// PCM 16 бит нужной частоты/каналов от читателя (с запасным вариантом «как есть»).
fn set_pcm(r: &IMFSourceReader, rate: u32, ch: u32) -> Result<(u32, u32)> {
    unsafe {
        let t = new_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch * 2)?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * ch * 2)?;
        if r.SetCurrentMediaType(FIRST_AUDIO, None, &t).is_err() {
            // частоту не поменять — берём PCM 16 бит в исходной частоте
            let p = new_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
            p.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            r.SetCurrentMediaType(FIRST_AUDIO, None, &p)?;
        }
        let cur = r.GetCurrentMediaType(FIRST_AUDIO)?;
        Ok((cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?, cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?))
    }
}

/// Выходной тип аудиокодировщика: из списка, что предлагает Windows, — ближайший по битрейту.
fn audio_out_type(sub: &GUID, rate: u32, ch: u32, kbps: u32) -> Result<IMFMediaType> {
    unsafe {
        let list = MFTranscodeGetAudioOutputAvailableTypes(sub, (MFT_ENUM_FLAG_ALL.0 & !MFT_ENUM_FLAG_FIELDOFUSE.0) as u32, None)?;
        let n = list.GetElementCount()?;
        let mut best: Option<(i64, IMFMediaType)> = None;
        for i in 0..n {
            let Ok(t) = list.GetElement(i).and_then(|u| u.cast::<IMFMediaType>()) else { continue };
            let r = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
            let c = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0);
            let b = t.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND).unwrap_or(0) * 8 / 1000;
            // частота и каналы должны совпасть; затем — ближайший битрейт
            let score = (r != rate) as i64 * 1_000_000 + (c != ch) as i64 * 100_000 + (b as i64 - kbps as i64).abs();
            if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
                best = Some((score, t));
            }
        }
        match best {
            Some((s, t)) if s < 100_000 => Ok(t),
            _ => Err(Error::new(windows::Win32::Foundation::E_FAIL, "кодировщик не поддерживает такую частоту/каналы")),
        }
    }
}

fn av(src: &Path, dst: &Path, kind: Kind, s: &Settings, progress: &dyn Fn(f64), stop: &AtomicBool) -> std::result::Result<(), String> {
    let ext = crate::util::ext_lower(dst);
    let want_video = kind == Kind::Video && matches!(ext.as_str(), "mp4" | "wmv");
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2).map_err(err)?;
        let attrs = attrs.unwrap();
        let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1);
        let url = crate::util::wpath(src);
        let r = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attrs).map_err(err)?;
        let dur = duration(&r).max(1);
        r.SetStreamSelection(ALL, false).map_err(err)?;
        let has_audio = r.SetStreamSelection(FIRST_AUDIO, true).is_ok();
        let has_video = want_video && r.SetStreamSelection(FIRST_VIDEO, true).is_ok();
        if !has_audio && !has_video {
            return Err("в файле нет звука и видео".into());
        }
        // ---- звук
        let (q_kbps_aac, q_kbps_mp3) = [(192, 320), (160, 192), (128, 128)][s.quality.min(2)];
        let mut audio_in: Option<IMFMediaType> = None;
        let mut audio_out: Option<IMFMediaType> = None;
        if has_audio {
            let native = r.GetNativeMediaType(FIRST_AUDIO, 0).map_err(err)?;
            let nr = native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(44100);
            let nc = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).clamp(1, 2);
            let rate = if ext == "wav" || ext == "flac" { nr } else if nr == 48000 { 48000 } else { 44100 };
            let (rate, ch) = set_pcm(&r, rate, nc).map_err(err)?;
            audio_in = Some(r.GetCurrentMediaType(FIRST_AUDIO).map_err(err)?);
            audio_out = match ext.as_str() {
                "wav" => None,
                "mp3" => Some(audio_out_type(&MFAudioFormat_MP3, rate, ch, q_kbps_mp3).map_err(err)?),
                "wma" | "wmv" => Some(audio_out_type(&MFAudioFormat_WMAudioV9, rate, ch, q_kbps_aac).map_err(err)?),
                "flac" => Some(audio_out_type(&MFAudioFormat_FLAC, rate, ch, 0).map_err(err)?),
                _ => Some(audio_out_type(&MFAudioFormat_AAC, rate, ch, q_kbps_aac).map_err(err)?),
            };
        }
        if ext == "wav" {
            return wav(&r, dst, dur, progress, stop);
        }
        // ---- видео
        let mut video_in: Option<IMFMediaType> = None;
        let mut video_out: Option<IMFMediaType> = None;
        if has_video {
            let native = r.GetNativeMediaType(FIRST_VIDEO, 0).map_err(err)?;
            let (w0, h0) = unpack(native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?);
            let (fn_, fd) = native.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((30, 1));
            let (mut w, mut h) = (w0, h0);
            if s.video_h > 0 && h0 > s.video_h {
                h = s.video_h;
                w = (w0 as u64 * h as u64 / h0 as u64) as u32;
            }
            let (w, h) = (w & !1, h & !1);
            let fps = if fd > 0 { fn_ as f64 / fd as f64 } else { 30.0 };
            let bits = [0.14, 0.09, 0.055][s.quality.min(2)];
            let bitrate = ((w as f64 * h as f64 * fps.clamp(1.0, 120.0) * bits) as u32).clamp(300_000, 40_000_000);
            let sub = if ext == "wmv" { MFVideoFormat_WMV3 } else { MFVideoFormat_H264 };
            let out = new_type(&MFMediaType_Video, &sub).map_err(err)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(err)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fn_.max(1), fd.max(1))).map_err(err)?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(err)?;
            if ext == "mp4" {
                let _ = out.SetUINT32(&MF_MT_MPEG2_PROFILE, 77); // Main
            }
            let inp = new_type(&MFMediaType_Video, &MFVideoFormat_NV12).map_err(err)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(err)?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fn_.max(1), fd.max(1))).map_err(err)?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err)?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err)?;
            r.SetCurrentMediaType(FIRST_VIDEO, None, &inp).map_err(|e| format!("видео: {}", err(e)))?;
            video_in = Some(r.GetCurrentMediaType(FIRST_VIDEO).map_err(err)?);
            video_out = Some(out);
        }
        // ---- запись
        let mut wattrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut wattrs, 2).map_err(err)?;
        let wattrs = wattrs.unwrap();
        let _ = wattrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);
        let container = match ext.as_str() {
            "mp3" => MFTranscodeContainerType_MP3,
            "wma" | "wmv" => MFTranscodeContainerType_ASF,
            "flac" => MFTranscodeContainerType_FLAC,
            _ => MFTranscodeContainerType_MPEG4,
        };
        let _ = wattrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container);
        let durl = crate::util::wpath(dst);
        let wr = MFCreateSinkWriterFromURL(PCWSTR(durl.as_ptr()), None, &wattrs).map_err(err)?;
        let mut map: Vec<(u32, u32)> = vec![]; // (поток читателя, поток писателя)
        if let (Some(o), Some(i)) = (&video_out, &video_in) {
            let k = wr.AddStream(o).map_err(|e| format!("видео: {}", err(e)))?;
            wr.SetInputMediaType(k, i, None).map_err(|e| format!("видео: {}", err(e)))?;
            map.push((FIRST_VIDEO, k));
        }
        if let (Some(o), Some(i)) = (&audio_out, &audio_in) {
            let k = wr.AddStream(o).map_err(|e| format!("звук: {}", err(e)))?;
            wr.SetInputMediaType(k, i, None).map_err(|e| format!("звук: {}", err(e)))?;
            map.push((FIRST_AUDIO, k));
        }
        // настоящие номера потоков читателя
        let mut real: Vec<(Option<u32>, u32, bool)> = map.iter().map(|&(_, k)| (None, k, false)).collect();
        wr.BeginWriting().map_err(err)?;
        let kinds: Vec<bool> = map.iter().map(|(rs, _)| *rs == FIRST_VIDEO).collect();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("остановлено".into());
            }
            let (mut idx, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            r.ReadSample(ANY, 0, Some(&mut idx), Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(err)?;
            // сопоставить поток читателя с потоком писателя по типу
            let is_video = r.GetCurrentMediaType(idx).ok().and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE).ok()) == Some(MFMediaType_Video);
            let slot = kinds.iter().position(|&v| v == is_video);
            if let Some(si) = slot {
                real[si].0 = Some(idx);
                let k = real[si].1;
                if let Some(smp) = &sample {
                    wr.WriteSample(k, smp).map_err(err)?;
                }
                if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                    let _ = wr.SendStreamTick(k, ts);
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    real[si].2 = true;
                }
            }
            if ts > 0 {
                progress((ts as f64 / dur as f64).min(0.999));
            }
            if real.iter().all(|x| x.2) || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                break;
            }
        }
        wr.Finalize().map_err(err)?;
        progress(1.0);
        Ok(())
    }
}

/// WAV пишем сами: заголовок RIFF + PCM от читателя.
fn wav(r: &IMFSourceReader, dst: &Path, dur: u64, progress: &dyn Fn(f64), stop: &AtomicBool) -> std::result::Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};
    unsafe {
        let t = r.GetCurrentMediaType(FIRST_AUDIO).map_err(err)?;
        let rate = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).map_err(err)?;
        let ch = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(err)? as u16;
        let mut f = std::io::BufWriter::new(std::fs::File::create(dst).map_err(|e| e.to_string())?);
        let hdr = wav_header(rate, ch, 0);
        f.write_all(&hdr).map_err(|e| e.to_string())?;
        let mut total = 0u32;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err("остановлено".into());
            }
            let (mut flags, mut ts) = (0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            r.ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).map_err(err)?;
            if let Some(s) = sample {
                let buf = s.ConvertToContiguousBuffer().map_err(err)?;
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut n = 0u32;
                buf.Lock(&mut p, None, Some(&mut n)).map_err(err)?;
                f.write_all(std::slice::from_raw_parts(p, n as usize)).map_err(|e| e.to_string())?;
                let _ = buf.Unlock();
                total = total.saturating_add(n);
            }
            progress((ts as f64 / dur as f64).min(0.999));
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        f.write_all(&wav_header(rate, ch, total)).map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        progress(1.0);
        Ok(())
    }
}

pub fn wav_header(rate: u32, ch: u16, data_len: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&ch.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
    h.extend_from_slice(&(ch * 2).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let d = std::env::temp_dir().join(format!("spv_conv_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("видео.mov");
        let p = out_path(&src, "mp4", None);
        assert_eq!(p, d.join("видео.mp4"));
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(out_path(&src, "mp4", None), d.join("видео (1).mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wav_hdr() {
        let h = wav_header(44100, 2, 400);
        assert_eq!(h.len(), 44);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 436);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 176400);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 400);
    }

    #[test]
    fn targets() {
        let s = Settings { photo: 1, photo_max: 0, audio: 3, video: 2, video_h: 0, quality: 0 };
        assert_eq!(target_ext(Kind::Image, &s), "png");
        assert_eq!(target_ext(Kind::Audio, &s), "flac");
        assert_eq!(target_ext(Kind::Video, &s), "mp3");
    }
}
