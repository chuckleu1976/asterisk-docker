-- Add transcript TEXT column to calls table for Whisper speech-to-text output
ALTER TABLE calls ADD COLUMN transcript TEXT;
