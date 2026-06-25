use anyhow::{bail, Context, Result};
use log::{debug, error};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

/// Convert an AMR audio blob to text using a local whisper.cpp installation.
///
/// Steps:
/// 1. Write `amr_bytes` to `<tmpdir>/<id>.amr`
/// 2. Run ffmpeg to convert to 16 kHz mono WAV: `<tmpdir>/<id>.wav`
/// 3. Run whisper-cli with `--output-txt` (language auto-detected or forced)
/// 4. Read the generated `<tmpdir>/<id>.wav.txt` transcript
/// 5. Delete all temp files
///
/// `language`: language code ("en", "zh", "ja", etc.) or "auto" for auto-detect.
/// Returns the trimmed transcript string on success.
pub async fn transcribe(
    amr_bytes: &[u8],
    ffmpeg_exe: &str,
    whisper_exe: &str,
    whisper_model: &str,
    language: &str,
) -> Result<String> {
    let tmp = std::env::temp_dir();
    let id = Uuid::new_v4().to_string();

    let amr_path: PathBuf = tmp.join(format!("{}.amr", id));
    let wav_path: PathBuf = tmp.join(format!("{}.wav", id));
    // whisper --output-txt writes to <-of value>.txt
    let txt_path: PathBuf = tmp.join(format!("{}.txt", id));

    // ── Step 1: Write AMR bytes ───────────────────────────────────────────────
    tokio::fs::write(&amr_path, amr_bytes)
        .await
        .context("failed to write temp AMR file")?;

    // ── Step 2: ffmpeg AMR → 16 kHz mono WAV ─────────────────────────────────
    let ffmpeg_child = Command::new(ffmpeg_exe)
        .args([
            "-y",               // overwrite output without asking
            "-i", amr_path.to_str().unwrap(),
            "-ar", "16000",     // 16 kHz sample rate (required by Whisper)
            "-ac", "1",         // mono
            wav_path.to_str().unwrap(),
        ])
        .kill_on_drop(true)
        .spawn()
        .context("failed to launch ffmpeg (is it installed and on PATH?)")?;

    let ffmpeg_out = match tokio::time::timeout(
        Duration::from_secs(60),
        ffmpeg_child.wait_with_output(),
    ).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            cleanup(&[&amr_path, &wav_path, &txt_path]).await;
            bail!("ffmpeg error: {}", e);
        }
        Err(_) => {
            cleanup(&[&amr_path, &wav_path, &txt_path]).await;
            bail!("ffmpeg timed out after 60s");
        }
    };

    if !ffmpeg_out.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_out.stderr);
        cleanup(&[&amr_path, &wav_path, &txt_path]).await;
        bail!("ffmpeg failed: {}", stderr.trim());
    }

    debug!("transcribe: ffmpeg ok, wav={}", wav_path.display());

    // ── Step 3: whisper-cli transcription ─────────────────────────────────────
    // Build args dynamically: omit "-l" when language is "auto" so Whisper
    // performs its own language detection pass.
    let output_prefix = txt_path.with_extension("");
    let output_prefix_str = output_prefix.to_str().unwrap();
    let wav_str = wav_path.to_str().unwrap();
    let mut whisper_args: Vec<&str> = vec![
        "-m", whisper_model,
        "-f", wav_str,
        "--output-txt",     // write transcript to <-of>.txt
        "-nt",              // no timestamps in output
        "-of", output_prefix_str,
    ];
    if language != "auto" {
        whisper_args.push("-l");
        whisper_args.push(language);
    }
    let whisper_child = Command::new(whisper_exe)
        .args(&whisper_args)
        .env("LD_LIBRARY_PATH", "/home/ht/.local/lib")
        .env("GGML_BACKEND_PATH", "/home/ht/.local/lib/libggml-cpu-haswell.so")
        .kill_on_drop(true)
        .spawn()
        .context("failed to launch whisper-cli (is it installed?)")?;

    let whisper_out = match tokio::time::timeout(
        Duration::from_secs(120),
        whisper_child.wait_with_output(),
    ).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            cleanup(&[&amr_path, &wav_path, &txt_path]).await;
            bail!("whisper-cli error: {}", e);
        }
        Err(_) => {
            cleanup(&[&amr_path, &wav_path, &txt_path]).await;
            bail!("whisper-cli timed out after 120s");
        }
    };

    if !whisper_out.status.success() {
        let stderr = String::from_utf8_lossy(&whisper_out.stderr);
        cleanup(&[&amr_path, &wav_path, &txt_path]).await;
        bail!("whisper-cli failed: {}", stderr.trim());
    }

    debug!("transcribe: whisper ok, reading {}", txt_path.display());

    // ── Step 4: Read transcript ───────────────────────────────────────────────
    let transcript = tokio::fs::read_to_string(&txt_path)
        .await
        .context("failed to read whisper transcript file")?;

    // ── Step 5: Cleanup ───────────────────────────────────────────────────────
    cleanup(&[&amr_path, &wav_path, &txt_path]).await;

    Ok(transcript.trim().to_string())
}

async fn cleanup(paths: &[&PathBuf]) {
    for p in paths {
        if let Err(e) = tokio::fs::remove_file(p).await {
            // Non-fatal — just log at debug level
            error!("transcribe: failed to delete temp file {}: {}", p.display(), e);
        }
    }
}
