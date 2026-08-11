use anyhow::Context;
use matroska_demuxer::{Frame, MatroskaFile, TrackType};
use opus_decoder::OpusDecoder;
use std::io::Cursor;

const WAV_SAMPLE_RATE: u32 = 16_000;
const WAV_CHANNELS: u16 = 1;
const WAV_BITS_PER_SAMPLE: u16 = 16;

pub fn transcode_webm_opus_to_wav(webm: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut container =
        MatroskaFile::open(Cursor::new(webm)).context("无法解析浏览器录制的 WebM 音频")?;
    let track = container
        .tracks()
        .iter()
        .find(|track| track.track_type() == TrackType::Audio)
        .ok_or_else(|| anyhow::anyhow!("WebM 中未找到音频轨道"))?;
    if track.codec_id() != "A_OPUS" {
        anyhow::bail!("WebM 音频编码不是 Opus: {}", track.codec_id());
    }
    let channel_count = track
        .audio()
        .map(|audio| audio.channels().get() as usize)
        .unwrap_or(1);
    if !(1..=2).contains(&channel_count) {
        anyhow::bail!("暂不支持 {channel_count} 声道的 Opus 音频");
    }
    let track_number = track.track_number().get();
    let pre_skip = opus_pre_skip_samples(track.codec_private(), WAV_SAMPLE_RATE);
    let mut decoder =
        OpusDecoder::new(WAV_SAMPLE_RATE, channel_count).context("无法初始化 Opus 解码器")?;
    let mut decoded = vec![0i16; decoder.max_frame_size_per_channel() * channel_count];
    let mut mono_samples = Vec::new();
    let mut frame = Frame::default();

    while container
        .next_frame(&mut frame)
        .context("读取 WebM Opus 数据帧失败")?
    {
        if frame.track != track_number {
            continue;
        }
        let samples_per_channel = decoder
            .decode(&frame.data, &mut decoded, false)
            .context("解码 WebM Opus 数据帧失败")?;
        let sample_count = samples_per_channel * channel_count;
        if channel_count == 1 {
            mono_samples.extend_from_slice(&decoded[..sample_count]);
        } else {
            mono_samples.extend(
                decoded[..sample_count]
                    .chunks_exact(2)
                    .map(|pair| ((i32::from(pair[0]) + i32::from(pair[1])) / 2) as i16),
            );
        }
    }

    if mono_samples.len() <= pre_skip {
        anyhow::bail!("WebM Opus 音频没有可转写的有效采样");
    }
    encode_pcm_wav(&mono_samples[pre_skip..])
}

fn opus_pre_skip_samples(codec_private: Option<&[u8]>, output_sample_rate: u32) -> usize {
    let Some(header) = codec_private.filter(|header| header.len() >= 12) else {
        return 0;
    };
    if &header[..8] != b"OpusHead" {
        return 0;
    }
    let pre_skip_48k = u16::from_le_bytes([header[10], header[11]]) as u64;
    ((pre_skip_48k * u64::from(output_sample_rate)) / 48_000) as usize
}

fn encode_pcm_wav(samples: &[i16]) -> anyhow::Result<Vec<u8>> {
    let data_size = samples
        .len()
        .checked_mul(std::mem::size_of::<i16>())
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| anyhow::anyhow!("转码后的 WAV 音频过大"))?;
    let riff_size = data_size
        .checked_add(36)
        .ok_or_else(|| anyhow::anyhow!("转码后的 WAV 音频过大"))?;
    let byte_rate = WAV_SAMPLE_RATE * u32::from(WAV_CHANNELS) * u32::from(WAV_BITS_PER_SAMPLE) / 8;
    let block_align = WAV_CHANNELS * WAV_BITS_PER_SAMPLE / 8;
    let mut wav = Vec::with_capacity(data_size as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&WAV_CHANNELS.to_le_bytes());
    wav.extend_from_slice(&WAV_SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&WAV_BITS_PER_SAMPLE.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(wav)
}
