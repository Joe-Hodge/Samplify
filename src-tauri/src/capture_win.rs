use crate::{capture::RingBuffer, spotify_buffer::SpotifyBuffer};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        RemoteDesktop::ProcessIdToSessionId,
    },
};

pub const RATE: u32 = 48000;
pub const CHANNELS: u16 = 2;

pub struct WindowsAudio {
    pub spotify: Arc<Mutex<SpotifyBuffer>>,
    pub system_enabled: Arc<AtomicBool>,
}

impl WindowsAudio {
    pub fn start(system: Arc<Mutex<RingBuffer>>, seconds: f64, enabled: bool) -> Result<Self> {
        let spotify = Arc::new(Mutex::new(SpotifyBuffer::new(RATE, CHANNELS, seconds)));
        let system_enabled = Arc::new(AtomicBool::new(enabled));
        let buffer = spotify.clone();
        worker("Spotify audio", move || {
            let result = (|| {
                let pid = spotify_root()?
                    .ok_or_else(|| anyhow!("Open Spotify on this PC to start capturing."))?;
                let client = AudioClient::new_application_loopback_client(pid, true)
                    .map_err(|e| anyhow!("Windows could not isolate Spotify audio ({e}). Application capture requires Windows build 20348 or later."))?;
                buffer.lock().unwrap().unavailable = None;
                capture(
                    client,
                    |pcm, end, gap| buffer.lock().unwrap().push(pcm, end, gap),
                    || Ok(spotify_root()? == Some(pid)),
                )
            })();
            let mut b = buffer.lock().unwrap();
            b.clear_audio();
            b.unavailable = Some(
                result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Reconnecting to Spotify audio.".into()),
            );
        })?;
        let active = system_enabled.clone();
        worker("System audio", move || {
            if !active.load(Ordering::Acquire) {
                return;
            }
            let result = (|| {
                let devices = DeviceEnumerator::new()?;
                let device = devices.get_default_device(Direction::Render)?;
                let id = device.get_id()?;
                capture(
                    device.get_iaudioclient()?,
                    |pcm, end, gap| {
                        let mut ring = system.lock().unwrap();
                        if gap {
                            ring.clear();
                        }
                        if active.load(Ordering::Acquire) {
                            ring.push_timed(pcm, end);
                        }
                    },
                    || {
                        Ok(active.load(Ordering::Acquire)
                            && devices.get_default_device(Direction::Render)?.get_id()? == id)
                    },
                )
            })();
            system.lock().unwrap().clear();
            if let Err(e) = result {
                log::warn!("System audio disconnected: {e}");
            }
        })?;
        Ok(Self {
            spotify,
            system_enabled,
        })
    }
}

fn worker(name: &str, mut attempt: impl FnMut() + Send + 'static) -> Result<()> {
    thread::Builder::new().name(name.into()).spawn(move || {
        if let Err(e) = wasapi::initialize_mta().ok() {
            log::error!("Audio COM initialization failed: {e}");
            return;
        }
        loop {
            attempt();
            thread::sleep(Duration::from_millis(600));
        }
    })?;
    Ok(())
}

// Only the root includes Spotify's renderer and audio utility processes.
// Never climb into Explorer or another application's process tree.
fn spotify_root() -> Result<Option<u32>> {
    unsafe {
        let mut session = 0;
        if ProcessIdToSessionId(std::process::id(), &mut session) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut more = Process32FirstW(snap, &mut entry);
        let mut spotify = HashMap::new();
        while more != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("Spotify.exe")
            {
                let mut owner = 0;
                if ProcessIdToSessionId(entry.th32ProcessID, &mut owner) != 0 && owner == session {
                    spotify.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                }
            }
            more = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        let mut roots = spotify
            .iter()
            .filter(|(_, parent)| !spotify.contains_key(parent))
            .map(|(&pid, _)| pid);
        let root = roots.next();
        if roots.next().is_some() {
            return Err(anyhow!(
                "Multiple Spotify players are open. Close the extra player and retry."
            ));
        }
        Ok(root)
    }
}

// WASAPI timestamps the first frame in QPC units of 100 ns. Use the actual
// packet end so a delayed callback cannot relabel queued audio after a seek.
fn packet_end(timestamp: u64, frames: u32, frequency: i64, now: Instant) -> Instant {
    let mut counter = 0;
    if timestamp == 0 || frequency <= 0 || unsafe { QueryPerformanceCounter(&mut counter) } == 0 {
        return now;
    }
    let age = counter as f64 / frequency as f64
        - timestamp as f64 / 10_000_000.0
        - frames as f64 / RATE as f64;
    now.checked_sub(Duration::from_secs_f64(age.max(0.0)))
        .unwrap_or(now)
}

fn capture(
    mut client: AudioClient,
    mut packet: impl FnMut(&[f32], Instant, bool),
    mut current: impl FnMut() -> Result<bool>,
) -> Result<()> {
    let format = WaveFormat::new(
        32,
        32,
        SampleType::Float,
        RATE as usize,
        CHANNELS as usize,
        None,
    );
    client.initialize_client(
        &format,
        Direction::Capture,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        },
        None,
    )?;
    let event = client.set_get_eventhandle()?;
    let reader = client.get_audiocaptureclient()?;
    let mut frequency = 0;
    unsafe {
        QueryPerformanceFrequency(&mut frequency);
    }
    let mut bytes = Vec::new();
    let mut samples = Vec::new();
    let mut check = Instant::now();
    client.start_stream()?;
    loop {
        // Idle renderers may send no events. Still check for a new device/PID.
        let _ = event.wait_for_event(200);
        if check.elapsed() >= Duration::from_secs(1) {
            if !current()? {
                break;
            }
            check = Instant::now();
        }
        while let Some(size) = reader.get_next_packet_size()?.filter(|&n| n > 0) {
            bytes.resize(size as usize * format.get_blockalign() as usize, 0);
            let (frames, info) = reader.read_from_device(&mut bytes)?;
            if frames == 0 {
                break;
            }
            samples.clear();
            samples.extend(
                bytes[..frames as usize * 8]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap())),
            );
            let now = Instant::now();
            let end = if info.flags.timestamp_error {
                now
            } else {
                packet_end(info.timestamp, frames, frequency, now)
            };
            packet(&samples, end, info.flags.data_discontinuity);
        }
    }
    client.stop_stream()?;
    Ok(())
}
