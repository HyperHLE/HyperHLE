/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Quick-and-dirty decoding of miscellaneous formats (MP3, AAC, CAF) to linear
//! PCM.
//!
//! This should be the only module in touchHLE that makes use of [symphonia].

// ============================================================
// ИСПРАВЛЕНИЯ (относительно исходника):
//
// 1. probe() — сигнатура в Symphonia 0.6 изменилась:
//      СТАРОЕ: get_probe().probe(&Default::default(), mss,
//                               Default::default(), Default::default())
//              возвращает ProbeResult (содержит fields tracks(), next_packet() и т.д.
//              непосредственно)
//      НОВОЕ:  get_probe().probe(&hint, mss, fmt_opts, meta_opts)
//              возвращает ProbeResult { format: Box<dyn FormatReader>, .. }
//              Дальнейший доступ к трекам/пакетам — через probed.format.
//
// 2. Доступ к трекам:
//      СТАРОЕ: probed.tracks() — нет такого метода у ProbeResult
//      НОВОЕ:  probed.format.tracks()
//
// 3. Поле codec_params у Track в Symphonia 0.6:
//      СТАРОЕ: track.codec_params — Option<CodecParameters>, метод .audio()
//              возвращает Option<&AudioCodecParameters>
//      НОВОЕ:  track.codec_params — CodecParameters (не Option!),
//              поле .codec: CodecType, сравниваем с CODEC_TYPE_NULL
//              Для создания декодера передаём &track.codec_params напрямую.
//
// 4. Создание декодера:
//      СТАРОЕ: get_codecs().make_audio_decoder(audio_codec_params, &Default::default())
//      НОВОЕ:  get_codecs().make(&track.codec_params, &DecoderOptions::default())
//
// 5. Цикл чтения пакетов:
//      СТАРОЕ: probed.next_packet() — нет такого метода у ProbeResult
//      НОВОЕ:  probed.format.next_packet() → Result<Packet, Error>
//              Конец потока — Error::IoError (UnexpectedEof) или специальный маркер;
//              начиная с 0.5 next_packet() возвращает Result<Packet>, ошибка IoError
//              трактуется как конец.
//
// 6. Экспорт PCM-семплов через RawSampleBuffer / SampleBuffer:
//      СТАРОЕ: decoded_packet.copy_bytes_to_vec_interleaved_as::<i16>(buf)
//              Метода copy_bytes_to_vec_interleaved_as не существует.
//      НОВОЕ:  Создаём SampleBuffer::<i16> один раз (или пересоздаём при смене spec),
//              вызываем buf.copy_interleaved_ref(audio_buf),
//              забираем семплы через buf.samples() и конвертируем в little-endian байты.
//
// 7. AudioSpec (несуществующий тип):
//      СТАРОЕ: use symphonia::core::audio::AudioSpec; + audio_spec.rate() + .channels().count()
//      НОВОЕ:  SignalSpec — правильный тип. Поля: spec.rate (u32), spec.channels (Channels).
//              Метод channels.count() существует и возвращает usize.
// ============================================================

use std::io::Cursor;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// PCM data decoded from a miscellaneous format file.
pub struct SymphoniaDecodedToPcm {
    /// 16-bit little-endian PCM samples, grouped in frames (one sample per
    /// channel in each frame).
    pub bytes: Vec<u8>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u32,
}

pub fn decode_symphonia_to_pcm(file: Cursor<Vec<u8>>) -> Result<SymphoniaDecodedToPcm, ()> {
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Symphonia 0.6: probe() принимает (&Hint, mss, FormatOptions, MetadataOptions)
    // и возвращает ProbeResult { format, metadata }.
    let hint = Hint::new();
    let fmt_opts = FormatOptions::default();
    let meta_opts = MetadataOptions::default();

    let probed = match symphonia::default::get_probe().probe(&hint, mss, fmt_opts, meta_opts) {
        Ok(p) => p,
        Err(e) => {
            log!("Symphonia probe failed: {:?}", e);
            return Err(());
        }
    };

    // В Symphonia 0.6 format reader доступен через probed.format.
    let mut format = probed.format;

    // Ищем первый аудиотрек с известным кодеком.
    // В 0.6 codec_params — CodecParameters (не Option), сравниваем .codec с CODEC_TYPE_NULL.
    let track = match format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
    {
        Some(t) => t,
        None => {
            log!("Symphonia: no supported audio tracks found in file");
            return Err(());
        }
    };

    let track_id = track.id;
    // Клонируем codec_params, чтобы не удерживать заимствование на format.
    let codec_params = track.codec_params.clone();

    // Создаём декодер через get_codecs().make(&codec_params, &dec_opts).
    let dec_opts = DecoderOptions::default();
    let mut decoder = match symphonia::default::get_codecs().make(&codec_params, &dec_opts) {
        Ok(d) => d,
        Err(e) => {
            log!("Symphonia failed to create decoder: {:?}", e);
            return Err(());
        }
    };

    let mut out_pcm = Vec::<u8>::new();
    // SampleBuffer пересоздаётся при первом декодированном пакете.
    let mut sample_buf: Option<SampleBuffer<i16>> = None;
    let mut out_spec: Option<symphonia::core::audio::SignalSpec> = None;

    loop {
        // format.next_packet() в Symphonia 0.6 возвращает Result<Packet, Error>.
        // IoError трактуется как нормальное завершение потока.
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::ResetRequired) => {
                // Цепочечный OGG или похожие форматы — просто останавливаемся.
                break;
            }
            Err(e) => {
                log!("Symphonia packet read error: {:?} (stopping decode)", e);
                break;
            }
        };

        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(buf) => buf,
            Err(SymphoniaError::DecodeError(e)) => {
                // Повреждённый фрейм — пропускаем, продолжаем.
                log!("Symphonia decode error (recoverable): {:?}", e);
                continue;
            }
            Err(e) => {
                log!("Symphonia fatal decode error: {:?}", e);
                break;
            }
        };

        // Запоминаем spec из первого успешно декодированного пакета.
        let spec = *decoded.spec();
        if out_spec.is_none() {
            out_spec = Some(spec);
        }

        // Создаём или пересоздаём SampleBuffer при смене параметров сигнала.
        let need_new_buf = sample_buf
            .as_ref()
            .map(|b| b.capacity() < decoded.capacity())
            .unwrap_or(true);
        if need_new_buf {
            sample_buf = Some(SampleBuffer::<i16>::new(decoded.capacity() as u64, spec));
        }

        let buf = sample_buf.as_mut().unwrap();
        // Копируем все каналы в перемежающийся порядок с конвертацией в i16.
        buf.copy_interleaved_ref(decoded);

        // Конвертируем i16 семплы в little-endian байты.
        for &s in buf.samples() {
            out_pcm.extend_from_slice(&s.to_le_bytes());
        }
    }

    let spec = match out_spec {
        Some(s) => s,
        None => {
            log!("Symphonia: file yielded no valid audio data");
            return Err(());
        }
    };

    if out_pcm.is_empty() {
        log!("Symphonia: decoded PCM buffer is empty");
        return Err(());
    }

    Ok(SymphoniaDecodedToPcm {
        bytes: out_pcm,
        sample_rate: spec.rate,
        channels: spec.channels.count().try_into().unwrap(),
    })
}

