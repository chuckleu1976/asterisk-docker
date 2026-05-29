use anyhow::{bail, Context, Result};
use log::{debug, error};
use std::path::PathBuf;
use tokio::process::Command;
use uuid::Uuid;

/// Convert an AMR audio blob to text using a local whisper.cpp installation.
///
/// Steps:
/// 1. Write `amr_bytes` to `<tmpdir>/<id>.amr`
/// 2. Run ffmpeg to convert to 16 kHz mono WAV: `<tmpdir>/<id>.wav`
/// 3. Run whisper-cli with English-only mode and `--output-txt`
/// 4. Read the generated `<tmpdir>/<id>.wav.txt` transcript
/// 5. Delete all temp files
///
/// Returns the trimmed transcript string on success.
pub async fn transcribe(
    amr_bytes: &[u8],
    ffmpeg_exe: &str,
    whisper_exe: &str,
    whisper_model: &str,
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
    let ffmpeg_status = Command::new(ffmpeg_exe)
        .args([
            "-y",               // overwrite output without asking
            "-i", amr_path.to_str().unwrap(),
            "-ar", "16000",     // 16 kHz sample rate (required by Whisper)
            "-ac", "1",         // mono
            wav_path.to_str().unwrap(),
        ])
        .output()
        .await
        .context("failed to launch ffmpeg (is it installed and on PATH?)")?;

    if !ffmpeg_status.status.success() {
        let stderr = String::from_utf8_lossy(&ffmpeg_status.stderr);
        cleanup(&[&amr_path, &wav_path, &txt_path]).await;
        bail!("ffmpeg failed: {}", stderr.trim());
    }

    debug!("transcribe: ffmpeg ok, wav={}", wav_path.display());

    // ── Step 3: whisper-cli transcription ─────────────────────────────────────
    let whisper_status = Command::new(whisper_exe)
        .args([
            "-m", whisper_model,
            "-l", "en",         // English only
            "-f", wav_path.to_str().unwrap(),
            "--output-txt",     // write transcript to <-of>.txt
            "-nt",              // no timestamps in output
            "-of", txt_path.with_extension("").to_str().unwrap(), // output path without .txt ext
        ])
        .output()
        .await
        .context("failed to launch whisper-cli (is it installed?)")?;

    if !whisper_status.status.success() {
        let stderr = String::from_utf8_lossy(&whisper_status.stderr);
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
