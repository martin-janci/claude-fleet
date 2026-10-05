//! The desktop as a voice source: the microphone (`capture`) converted to the
//! relay's PCM (`resample`).

// Nothing calls into these until the voice commands are wired up.
#[allow(dead_code)]
mod capture;
#[allow(dead_code)]
mod resample;
