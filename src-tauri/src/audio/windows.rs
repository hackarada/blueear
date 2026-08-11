//! Windows WASAPI process-loopback + microphone capture.
//!
//! SECURITY-REVIEW: meeting capture targets only allowlisted Teams/Zoom
//! executable path fragments. The UI never supplies arbitrary PIDs or paths.
//!
//! See `docs/superpowers/specs/2026-08-10-windows-process-loopback-design.md`
//! and `spike/windows-capture-spike/` for the activation contract.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use super::meeting_app::MeetingAppId;
use super::ring::RingProducer;
use super::status::StatusEvent;

/// Windows 10 build that first shipped process loopback.
const MIN_WINDOWS_BUILD: u32 = 20348;

struct CaptureHandles {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

struct EngineInner {
    meeting_tx: Mutex<Option<RingProducer>>,
    mic_tx: Mutex<Option<RingProducer>>,
    status_tx: mpsc::Sender<(StatusEvent, i32)>,
    meeting_capture: Mutex<Option<CaptureHandles>>,
    mic_capture: Mutex<Option<CaptureHandles>>,
}

pub struct WindowsAudioEngine {
    inner: Arc<EngineInner>,
}

impl WindowsAudioEngine {
    pub fn new() -> (Self, mpsc::Receiver<(StatusEvent, i32)>) {
        let (status_tx, status_rx) = mpsc::channel();
        let inner = Arc::new(EngineInner {
            meeting_tx: Mutex::new(None),
            mic_tx: Mutex::new(None),
            status_tx,
            meeting_capture: Mutex::new(None),
            mic_capture: Mutex::new(None),
        });
        (Self { inner }, status_rx)
    }

    pub fn os_supported(&self) -> bool {
        native::windows_build_number()
            .map(|build| build >= MIN_WINDOWS_BUILD)
            .unwrap_or(false)
    }

    pub fn is_meeting_app_running(&self, app: MeetingAppId) -> bool {
        find_meeting_pid(app).is_some()
    }

    pub fn is_meeting_app_installed(&self, app: MeetingAppId) -> bool {
        meeting_install_paths(app).iter().any(|p| p.exists())
            || find_meeting_pid(app).is_some()
    }

    pub fn probe_audio_permission(&self) -> bool {
        // Process loopback has no TCC analogue; meeting capture is allowed when
        // the OS build supports the API. Mic privacy is enforced when starting
        // the microphone track.
        self.os_supported()
    }

    pub fn microphone_input_available(&self) -> bool {
        native::default_capture_available()
    }

    pub fn start_meeting_capture(
        &self,
        app: MeetingAppId,
        producer: RingProducer,
    ) -> Result<(), i32> {
        let pid = find_meeting_pid(app).ok_or(-2)?;
        *self.inner.meeting_tx.lock().expect("meeting_tx") = Some(producer);

        let stop = Arc::new(AtomicBool::new(false));
        let inner = Arc::clone(&self.inner);
        let stop_flag = Arc::clone(&stop);
        let join = thread::spawn(move || {
            let _ = inner.status_tx.send((StatusEvent::SourceTapStarted, 0));
            let result =
                native::run_process_loopback(pid, &stop_flag, |samples, frames, ch, rate| {
                    push_pcm(&inner.meeting_tx, samples, frames, ch, rate);
                });
            match result {
                Ok(()) => {
                    let _ = inner.status_tx.send((StatusEvent::SourceTapStopped, 0));
                }
                Err(code) => {
                    let _ = inner.status_tx.send((StatusEvent::GenericError, code));
                }
            }
        });

        *self.inner.meeting_capture.lock().expect("meeting_capture") =
            Some(CaptureHandles {
                stop,
                join: Some(join),
            });
        Ok(())
    }

    pub fn stop_meeting_capture(&self) {
        stop_capture(&self.inner.meeting_capture);
        *self.inner.meeting_tx.lock().expect("meeting_tx") = None;
    }

    pub fn start_microphone_capture(&self, producer: RingProducer) -> Result<(), i32> {
        *self.inner.mic_tx.lock().expect("mic_tx") = Some(producer);
        let stop = Arc::new(AtomicBool::new(false));
        let inner = Arc::clone(&self.inner);
        let stop_flag = Arc::clone(&stop);
        let join = thread::spawn(move || {
            let _ = inner.status_tx.send((StatusEvent::MicStarted, 0));
            match native::run_microphone_capture(&stop_flag, |samples, frames, ch, rate| {
                push_pcm(&inner.mic_tx, samples, frames, ch, rate);
            }) {
                Ok(()) => {
                    let _ = inner.status_tx.send((StatusEvent::MicStopped, 0));
                }
                Err(code) => {
                    let _ = inner
                        .status_tx
                        .send((StatusEvent::AudioPermissionDenied, code));
                }
            }
        });
        *self.inner.mic_capture.lock().expect("mic_capture") = Some(CaptureHandles {
            stop,
            join: Some(join),
        });
        Ok(())
    }

    pub fn stop_microphone_capture(&self) {
        stop_capture(&self.inner.mic_capture);
        *self.inner.mic_tx.lock().expect("mic_tx") = None;
    }
}

impl Drop for WindowsAudioEngine {
    fn drop(&mut self) {
        self.stop_meeting_capture();
        self.stop_microphone_capture();
    }
}

fn stop_capture(slot: &Mutex<Option<CaptureHandles>>) {
    let mut guard = slot.lock().expect("capture slot");
    if let Some(mut handles) = guard.take() {
        handles.stop.store(true, Ordering::Relaxed);
        if let Some(join) = handles.join.take() {
            let _ = join.join();
        }
    }
}

fn push_pcm(
    slot: &Mutex<Option<RingProducer>>,
    samples: &[f32],
    frames: u32,
    channels: u32,
    sample_rate: f64,
) {
    let host_time_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if let Ok(mut guard) = slot.lock() {
        if let Some(producer) = guard.as_mut() {
            producer.push(samples, frames, channels, sample_rate, host_time_ns);
        }
    }
}

fn meeting_path_fragments(app: MeetingAppId) -> &'static [&'static str] {
    match app {
        MeetingAppId::Teams => &[
            r"\microsoft\teams\",
            r"\microsoft\windowsapps\ms-teams.exe",
            "ms-teams.exe",
            r"\teams.exe",
        ],
        MeetingAppId::Zoom => &[r"\zoom\bin\zoom.exe", r"\zoom.exe"],
    }
}

fn meeting_install_paths(app: MeetingAppId) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let pf86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);

    match app {
        MeetingAppId::Teams => {
            if let Some(local) = &local {
                paths.push(local.join(r"Microsoft\Teams\current\Teams.exe"));
                paths.push(local.join(r"Microsoft\WindowsApps\ms-teams.exe"));
            }
        }
        MeetingAppId::Zoom => {
            if let Some(roaming) = &roaming {
                paths.push(roaming.join(r"Zoom\bin\Zoom.exe"));
            }
            if let Some(pf) = &pf {
                paths.push(pf.join(r"Zoom\bin\Zoom.exe"));
            }
            if let Some(pf86) = &pf86 {
                paths.push(pf86.join(r"Zoom\bin\Zoom.exe"));
            }
        }
    }
    paths
}

fn path_matches_app(path: &Path, app: MeetingAppId) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    meeting_path_fragments(app)
        .iter()
        .any(|frag| lower.contains(&frag.to_ascii_lowercase()))
}

fn find_meeting_pid(app: MeetingAppId) -> Option<u32> {
    native::enumerate_process_paths()
        .into_iter()
        .find(|(_pid, path)| path_matches_app(path, app))
        .map(|(pid, _)| pid)
}

mod native {
    use super::*;

    pub fn windows_build_number() -> Option<u32> {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let key = hklm
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .ok()?;
        let build: String = key.get_value("CurrentBuild").ok()?;
        build.parse().ok()
    }

    pub fn enumerate_process_paths() -> Vec<(u32, PathBuf)> {
        use sysinfo::{ProcessesToUpdate, System};
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let path = process.exe()?.to_path_buf();
                Some((pid.as_u32(), path))
            })
            .collect()
    }

    pub fn default_capture_available() -> bool {
        // Presence of a default capture endpoint is enough for readiness; the
        // real open happens when mic capture starts.
        cpal::default_host()
            .default_input_device()
            .is_some()
    }

    pub fn run_process_loopback<F>(
        pid: u32,
        stop: &AtomicBool,
        on_pcm: F,
    ) -> Result<(), i32>
    where
        F: FnMut(&[f32], u32, u32, f64),
    {
        process_loopback::capture(pid, stop, on_pcm)
    }

    pub fn run_microphone_capture<F>(stop: &AtomicBool, on_pcm: F) -> Result<(), i32>
    where
        F: FnMut(&[f32], u32, u32, f64),
    {
        mic_capture::capture(stop, on_pcm)
    }

    mod process_loopback {
        use super::*;

        /// Activate WASAPI process loopback for `pid` and stream float PCM.
        ///
        /// Implemented in a dedicated submodule so the COM activation details
        /// stay out of the engine surface. Returns `-4` on tap creation failure
        /// (mapped by SessionManager).
        pub fn capture<F>(pid: u32, stop: &AtomicBool, mut on_pcm: F) -> Result<(), i32>
        where
            F: FnMut(&[f32], u32, u32, f64),
        {
            match loopback_wasapi::run(pid, stop, &mut on_pcm) {
                Ok(()) => Ok(()),
                Err(_) => Err(-4),
            }
        }

        mod loopback_wasapi {
            use super::*;
            use std::ptr;
            use std::sync::Mutex as StdMutex;

            use windows::core::{implement, Interface, Result as WinResult, GUID};
            use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
            use windows::Win32::Media::Audio::{
                ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
                IActivateAudioInterfaceCompletionHandler, IAudioCaptureClient, IAudioClient,
                AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
                PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, WAVEFORMATEX, WAVE_FORMAT_PCM,
            };
            use windows::Win32::System::Com::{
                CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
            };
            use windows::Win32::System::Threading::{
                CreateEventW, SetEvent, WaitForSingleObject, INFINITE,
            };
            use windows::Win32::System::Variant::{VARIANT, VT_BLOB};
            use windows::core::HRESULT;

            // VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK device string
            const PROCESS_LOOPBACK_DEVICE: windows::core::PCWSTR =
                windows::core::w!("VirtualAudioDeviceProcessLoopback");

            struct SharedClient {
                event: HANDLE,
                client: StdMutex<Option<IAudioClient>>,
                hr: StdMutex<HRESULT>,
            }

            #[implement(IActivateAudioInterfaceCompletionHandler)]
            struct Handler {
                shared: Arc<SharedClient>,
            }

            impl IActivateAudioInterfaceCompletionHandler_Impl for Handler {
                fn ActivateCompleted(
                    &self,
                    op: Option<&IActivateAudioInterfaceAsyncOperation>,
                ) -> WinResult<()> {
                    unsafe {
                        let mut activate_hr = HRESULT(0);
                        let mut unk = None;
                        if let Some(op) = op {
                            let _ = op.GetActivateResult(&mut activate_hr, &mut unk);
                        }
                        *self.shared.hr.lock().unwrap() = activate_hr;
                        if activate_hr.is_ok() {
                            if let Some(unk) = unk {
                                if let Ok(client) = unk.cast::<IAudioClient>() {
                                    *self.shared.client.lock().unwrap() = Some(client);
                                }
                            }
                        }
                        let _ = SetEvent(self.shared.event);
                    }
                    Ok(())
                }
            }

            pub fn run<F>(pid: u32, stop: &AtomicBool, on_pcm: &mut F) -> WinResult<()>
            where
                F: FnMut(&[f32], u32, u32, f64),
            {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let result = run_inner(pid, stop, on_pcm);
                    CoUninitialize();
                    result
                }
            }

            unsafe fn run_inner<F>(pid: u32, stop: &AtomicBool, on_pcm: &mut F) -> WinResult<()>
            where
                F: FnMut(&[f32], u32, u32, f64),
            {
                let event = CreateEventW(None, true, false, None)?;
                let shared = Arc::new(SharedClient {
                    event,
                    client: StdMutex::new(None),
                    hr: StdMutex::new(HRESULT(0)),
                });
                let handler: IActivateAudioInterfaceCompletionHandler = Handler {
                    shared: Arc::clone(&shared),
                }
                .into();

                let mut activation = AUDIOCLIENT_ACTIVATION_PARAMS {
                    ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                    Anonymous: Default::default(),
                };
                activation.Anonymous.ProcessLoopbackParams = AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                };

                let mut variant = VARIANT::default();
                // VT_BLOB holding AUDIOCLIENT_ACTIVATION_PARAMS
                (*variant.Anonymous.Anonymous).vt = VT_BLOB;
                (*variant.Anonymous.Anonymous).Anonymous.blob.cbSize =
                    std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
                (*variant.Anonymous.Anonymous).Anonymous.blob.pBlobData =
                    &mut activation as *mut _ as *mut u8;

                let mut async_op = None;
                ActivateAudioInterfaceAsync(
                    PROCESS_LOOPBACK_DEVICE,
                    &IAudioClient::IID,
                    Some(std::mem::transmute(&mut variant)),
                    &handler,
                    &mut async_op,
                )?;

                WaitForSingleObject(event, INFINITE);
                shared.hr.lock().unwrap().ok()?;
                let client = shared
                    .client
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or_else(|| windows::core::Error::from(HRESULT(0x80004005u32 as i32)))?;

                let format = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_PCM,
                    nChannels: 2,
                    nSamplesPerSec: 48000,
                    nAvgBytesPerSec: 48000 * 2 * 2,
                    nBlockAlign: 4,
                    wBitsPerSample: 16,
                    cbSize: 0,
                };
                let buffer_duration = 200_000i64; // 20 ms in 100-ns units
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                    buffer_duration,
                    0,
                    &format,
                    None,
                )?;

                let ready = CreateEventW(None, false, false, None)?;
                client.SetEventHandle(ready)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;

                let mut scratch = vec![0f32; 48_000];
                while !stop.load(Ordering::Relaxed) {
                    if WaitForSingleObject(ready, 100) != WAIT_OBJECT_0 {
                        continue;
                    }
                    loop {
                        let mut data_ptr = ptr::null_mut();
                        let mut frames = 0u32;
                        let mut flags = 0u32;
                        if capture
                            .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                            .is_err()
                            || frames == 0
                        {
                            break;
                        }
                        let channels = 2u32;
                        let sample_rate = 48_000.0f64;
                        let sample_count = frames as usize * channels as usize;
                        if sample_count > scratch.len() {
                            scratch.resize(sample_count, 0.0);
                        }
                        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                            scratch[..sample_count].fill(0.0);
                        } else if !data_ptr.is_null() {
                            let pcm =
                                std::slice::from_raw_parts(data_ptr as *const i16, sample_count);
                            for (i, s) in pcm.iter().enumerate() {
                                scratch[i] = f32::from(*s) / 32768.0;
                            }
                        }
                        on_pcm(&scratch[..sample_count], frames, channels, sample_rate);
                        let _ = capture.ReleaseBuffer(frames);
                    }
                }

                let _ = client.Stop();
                let _ = CloseHandle(ready);
                let _ = CloseHandle(event);
                let _ = (GUID::zeroed(), CLSCTX_ALL);
                Ok(())
            }
        }
    }

    mod mic_capture {
        use super::*;

        pub fn capture<F>(stop: &AtomicBool, mut on_pcm: F) -> Result<(), i32>
        where
            F: FnMut(&[f32], u32, u32, f64),
        {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

            let host = cpal::default_host();
            let device = host.default_input_device().ok_or(-1)?;
            let config = device.default_input_config().map_err(|_| -1)?;
            let sample_rate = config.sample_rate().0 as f64;
            let channels = u32::from(config.channels());

            let err_fn = |err| log::error!("microphone stream error: {err}");
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => {
                    let conf: cpal::StreamConfig = config.clone().into();
                    device
                        .build_input_stream(
                            &conf,
                            move |data: &[f32], _| {
                                let frames = (data.len() as u32) / channels.max(1);
                                on_pcm(data, frames, channels, sample_rate);
                            },
                            err_fn,
                            None,
                        )
                        .map_err(|_| -1)?
                }
                cpal::SampleFormat::I16 => {
                    let conf: cpal::StreamConfig = config.clone().into();
                    device
                        .build_input_stream(
                            &conf,
                            move |data: &[i16], _| {
                                let floats: Vec<f32> =
                                    data.iter().map(|s| f32::from(*s) / 32768.0).collect();
                                let frames = (floats.len() as u32) / channels.max(1);
                                on_pcm(&floats, frames, channels, sample_rate);
                            },
                            err_fn,
                            None,
                        )
                        .map_err(|_| -1)?
                }
                _ => return Err(-1),
            };

            stream.play().map_err(|_| -1)?;
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(std::time::Duration::from_millis(50));
            }
            drop(stream);
            Ok(())
        }
    }
}
