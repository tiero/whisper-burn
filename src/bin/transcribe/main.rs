use std::collections::HashMap;
use std::iter;

use whisper::model::*;
use whisper::helper::*;
use whisper::token;
use whisper::transcribe::waveform_to_text;

cfg_if::cfg_if! {
    if #[cfg(feature = "torch-backend")] {
        use burn_tch::{TchBackend, TchDevice};
    } else if #[cfg(feature = "wgpu-backend")] {
        use burn_wgpu::{WgpuBackend, WgpuDevice, AutoGraphicsApi};
    }
}

use burn::{
    config::Config, 
    module::Module, 
    tensor::{
        self, 
        backend::{self, Backend},
        Data, 
        Tensor,
        Int, 
        Float, 
    },
};

use hound::{self, SampleFormat};

fn load_audio_waveform<B: Backend>(filename: &str) -> hound::Result<(Vec<f32>, usize)> {
    let mut reader = hound::WavReader::open(filename)?;
    let spec = reader.spec();

    let duration = reader.duration() as usize;
    let channels = spec.channels as usize;
    let sample_rate = spec.sample_rate as usize;
    let bits_per_sample = spec.bits_per_sample;
    let sample_format = spec.sample_format;

    assert_eq!(sample_rate, 16000, "The audio sample rate must be 16k.");
    assert_eq!(channels, 1, "The audio must be single-channel.");

    let max_int_val = 2_u32.pow(spec.bits_per_sample as u32 - 1) - 1;

    let floats = match sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<hound::Result<_>>()?, 
        SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|s| s.map(|s| s as f32 / max_int_val as f32))
            .collect::<hound::Result<_>>()?, 
    };

    return Ok( (floats, sample_rate) );
}

use num_traits::ToPrimitive;
use whisper::audio::prep_audio;
use whisper::token::{Gpt2Tokenizer, SpecialToken};

use burn::record::{Recorder, DefaultRecorder, RecorderError};

fn load_whisper_model_file<B: Backend>(config: &WhisperConfig, filename: &str) -> Result<Whisper<B>, RecorderError> {
    DefaultRecorder::new()
    .load(filename.into())
    .map(|record| config.init().load_record(record))
}

use std::{env, process, fs};

fn main() {
    cfg_if::cfg_if! {
        if #[cfg(feature = "torch-backend")] {
            type Backend = TchBackend<f32>;
            let device = TchDevice::Cuda(0);
        } else if #[cfg(feature = "wgpu-backend")] {
            type Backend = WgpuBackend<AutoGraphicsApi, f32, i32>;
            let device = WgpuDevice::BestAvailable;
        }
    }

    let args: Vec<String> = env::args().collect();

    if args.len() < 4 {
        eprintln!("Usage: {} <model name> <audio file> <transcription file>", args[0]);
        process::exit(1);
    }

    let wav_file = &args[2];
    let text_file = &args[3];
    let model_name = &args[1];

    println!("Loading waveform...");
    let (waveform, sample_rate) = match load_audio_waveform::<Backend>(wav_file) {
        Ok( (w, sr) ) => (w, sr), 
        Err(e) => {
            eprintln!("Failed to load audio file: {}", e);
            process::exit(1);
        }
    };

    let bpe = match Gpt2Tokenizer::new() {
        Ok(bpe) => bpe, 
        Err(e) => {
            eprintln!("Failed to load tokenizer: {}", e);
            process::exit(1);
        }
    };

    let whisper_config = match WhisperConfig::load(&format!("{}.cfg", model_name)) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load whisper config: {}", e);
            process::exit(1);
        }
    };

    println!("Loading model...");
    let whisper: Whisper<Backend> = match load_whisper_model_file(&whisper_config, model_name) {
        Ok(whisper_model) => whisper_model,
        Err(e) => {
            eprintln!("Failed to load whisper model file: {}", e);
            process::exit(1);
        }
    };
    
    let whisper = whisper.to_device(&device);

    let (text, tokens) = match waveform_to_text(&whisper, &bpe, waveform, sample_rate) {
        Ok( (text, tokens) ) => (text, tokens), 
        Err(e) => {
            eprintln!("Error during transcription: {}", e);
            process::exit(1);
        }
    };

    fs::write(text_file, text).unwrap_or_else(|e| {
        eprintln!("Error writing transcription file: {}", e);
        process::exit(1);
    });

    println!("Transcription finished.");
}
