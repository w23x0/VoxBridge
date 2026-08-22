//! 手动验一遍这个 crate 到底通不通。跑法：
//!
//! ```text
//! cargo run -p vox-audio-win --example smoke              # 只查，不出声
//! cargo run -p vox-audio-win --example smoke -- mic       # 录 3 秒麦克风
//! cargo run -p vox-audio-win --example smoke -- mic "麦克风 (Realtek)"
//! cargo run -p vox-audio-win --example smoke -- tone      # 默认输出放 1 秒 440 Hz
//! cargo run -p vox-audio-win --example smoke -- tone "CABLE Input (VB-Audio Virtual Cable)"
//! cargo run -p vox-audio-win --example smoke -- app msedge.exe   # 抓某个程序的声音 5 秒
//! ```
//!
//! 只读系统状态 + 用默认/指定设备收发音。**不下载、不安装任何东西**，
//! VB-CABLE 那栏只报“装了没”。

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use vox_audio_win::{
    cable, os_build_number, process_loopback_available, WinCapture, WinDeviceRegistry, WinPlayback,
};
use vox_core::pipeline::ResampleFactory;
use vox_core::ports::{
    AudioChunk, CaptureSource, CaptureTarget, DeviceRegistry, PlaybackSink, PortResult,
};

/// 峰值用定点存进 AtomicU32（f32 不能原子累计），乘 10000 够看了。
const PEAK_SCALE: f32 = 10_000.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("devices");
    let arg = args.get(1).map(String::as_str);

    let result = match cmd {
        "devices" => survey(),
        "blockers" => list_cable_blockers(),
        "mic" => record_mic(arg),
        "tone" => play_tone(arg),
        "app" => record_app(arg),
        "selftest" => loopback_selftest(),
        other => {
            eprintln!(
                "不认识的命令 {other:?}。可用：devices / blockers / mic / tone / app / selftest"
            );
            std::process::exit(2);
        }
    };

    if let Err(err) = result {
        eprintln!("\x1b[31m失败：{err}\x1b[0m");
        std::process::exit(1);
    }
}

fn list_cable_blockers() -> PortResult<()> {
    let apps = vox_audio_win::virtual_cable_blocking_apps()?;
    if apps.is_empty() {
        println!("没有应用占用 VB-CABLE。 ");
    } else {
        for app in apps {
            println!("{} ({}, PID {})", app.display_name, app.executable, app.pid);
        }
    }
    Ok(())
}

/// 只查状态：系统版本、设备清单、正在出声的程序、VB-CABLE 在不在。
fn survey() -> PortResult<()> {
    let build = os_build_number();
    println!("系统内部版本：{build}");
    println!(
        "进程环回（按程序抓声音）：{}",
        if process_loopback_available() {
            "可用"
        } else {
            "不可用（需要 20348 及以上，即 Win11 / Server 2022）"
        }
    );

    let reg = WinDeviceRegistry::new();

    println!("\n输入设备：");
    print_devices(&reg.input_devices()?);
    println!("\n输出设备：");
    print_devices(&reg.output_devices()?);

    println!("\n正在放声音的程序：");
    let apps = reg.audio_apps()?;
    if apps.is_empty() {
        println!("  （一个都没有）");
    }
    for app in &apps {
        println!(
            "  {:<28} pid {:<7} {}",
            app.display_name,
            app.pid,
            if app.active { "出声中" } else { "静默" }
        );
    }

    println!("\nVB-CABLE：{}", describe_cable());
    Ok(())
}

fn print_devices(list: &[vox_core::ports::DeviceInfo]) {
    if list.is_empty() {
        println!("  （一个都没有）");
    }
    for d in list {
        println!("  {}{}", if d.is_default { "* " } else { "  " }, d.name);
    }
}

fn describe_cable() -> &'static str {
    // 纯检测。这个例子永远不下载、不安装。
    match cable::detect() {
        cable::CableStatus::Installed => "装好了，可以用",
        cable::CableStatus::InstalledPendingReboot => "装了但要重启才出现端点",
        cable::CableStatus::UninstallIncomplete => "卸载未完成，可关闭占用应用后重试",
        cable::CableStatus::NotInstalled => "没装",
    }
}

/// 录 3 秒麦克风，报块数和峰值。峰值一直是 0 就说明没真录到。
fn record_mic(device: Option<&str>) -> PortResult<()> {
    let target = CaptureTarget::Microphone(device.map(str::to_owned));
    println!("开始录 3 秒：{}", device.unwrap_or("（系统默认输入设备）"));
    run_capture(target, Duration::from_secs(3))
}

/// 抓指定程序（含子进程）放出来的声音 5 秒。
fn record_app(exe: Option<&str>) -> PortResult<()> {
    let Some(exe) = exe else {
        eprintln!("用法：... -- app <程序名.exe>，比如 app msedge.exe");
        std::process::exit(2);
    };
    if !process_loopback_available() {
        println!(
            "这台机器的内部版本 {} 不支持按程序抓声音。",
            os_build_number()
        );
        return Ok(());
    }
    println!("开始抓 {exe}（含子进程）5 秒。让它出点声音，否则一帧都不会来。");
    let target = CaptureTarget::ProcessLoopback {
        executable: exe.to_owned(),
        include_tree: true,
    };
    run_capture(target, Duration::from_secs(5))
}

/// 采集通用流程：起流、跑一段、停流，然后确认 stop 之后回调真的不再来。
fn run_capture(target: CaptureTarget, dur: Duration) -> PortResult<()> {
    let chunks = Arc::new(AtomicUsize::new(0));
    let frames = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicU32::new(0));

    let (c, f, p) = (Arc::clone(&chunks), Arc::clone(&frames), Arc::clone(&peak));
    let mut cap = WinCapture::new();
    let format = cap.start(
        &target,
        40,
        Box::new(move |chunk: AudioChunk| {
            c.fetch_add(1, Ordering::Relaxed);
            f.fetch_add(
                chunk.samples.len() / chunk.channels.max(1) as usize,
                Ordering::Relaxed,
            );
            let local = chunk.samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
            p.fetch_max((local * PEAK_SCALE) as u32, Ordering::Relaxed);
        }),
    )?;

    println!(
        "协商到的格式：{} Hz / {} 声道",
        format.sample_rate, format.channels
    );
    std::thread::sleep(dur);
    cap.stop();

    let after_stop = chunks.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    let settled = chunks.load(Ordering::Relaxed);

    let seconds = frames.load(Ordering::Relaxed) as f64 / format.sample_rate.max(1) as f64;
    println!(
        "收到 {} 块 / {:.2} 秒音频，峰值 {:.4}",
        after_stop,
        seconds,
        peak.load(Ordering::Relaxed) as f32 / PEAK_SCALE
    );
    if settled != after_stop {
        println!("\x1b[31mstop() 之后回调还在触发（{after_stop} → {settled}），这是 bug。\x1b[0m");
    } else {
        println!("stop() 之后回调没再触发。");
    }
    if peak.load(Ordering::Relaxed) == 0 {
        println!("峰值是 0：要么真的没声音，要么这条路没通。");
    }
    Ok(())
}

/// 往输出设备放 1 秒 440 Hz 正弦（内核推的就是 24 kHz 单声道 f32，这里照样推）。
fn play_tone(device: Option<&str>) -> PortResult<()> {
    const SOURCE_RATE: u32 = 24_000;
    // 简单透传：smoke 测的是 WASAPI 链路，不关心音质。
    struct Passthrough;
    impl vox_core::ports::Resample for Passthrough {
        fn process(&mut self, samples: &[f32]) -> Vec<f32> {
            samples.to_vec()
        }
        fn flush(&mut self) -> Vec<f32> {
            Vec::new()
        }
        fn reset(&mut self) {}
    }
    let factory: ResampleFactory = Box::new(|_, _| Box::new(Passthrough));
    let mut sink = WinPlayback::new(factory);
    let rate = sink.open(device, SOURCE_RATE)?;
    println!(
        "打开 {}，设备采样率 {rate} Hz{}",
        device.unwrap_or("（系统默认输出设备）"),
        if rate == SOURCE_RATE {
            "（不用重采样）"
        } else {
            "（会从 24 kHz 重采样）"
        }
    );

    // 分成 40 ms 一块推，跟内核的节奏一致。
    let block = (SOURCE_RATE / 25) as usize;
    let mut phase = 0.0f32;
    let step = std::f32::consts::TAU * 440.0 / SOURCE_RATE as f32;
    for _ in 0..25 {
        let mut buf = Vec::with_capacity(block);
        for _ in 0..block {
            buf.push(phase.sin() * 0.25);
            phase = (phase + step) % std::f32::consts::TAU;
        }
        sink.push(&buf);
        std::thread::sleep(Duration::from_millis(40));
    }
    // 等尾巴放完再关，不然最后一截会被截掉。
    std::thread::sleep(Duration::from_millis(300));
    sink.close();
    println!("放完了。听见蜂鸣就说明这条链通了。");
    Ok(())
}

/// 进程环回的闭环自测：一边自己放音，一边抓自己这个进程的声音。
///
/// 拿别的程序当目标验不准——它可能开着流但在暂停，抓到的全是驱动标了
/// SILENT 的零帧，分不清是“它没出声”还是“这条路没通”。自己放自己抓就没这个歧义：
/// 峰值非零就是真的通了。
fn loopback_selftest() -> PortResult<()> {
    if !process_loopback_available() {
        println!(
            "这台机器内部版本 {}，进程环回要 20348 及以上，跳过。",
            os_build_number()
        );
        return Ok(());
    }

    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "smoke.exe".to_string());
    println!("抓自己（{exe}）的声音，同时放 1 秒 440 Hz。会听见蜂鸣。");

    let chunks = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let (c, p) = (Arc::clone(&chunks), Arc::clone(&peak));

    let mut cap = WinCapture::new();
    let format = cap.start(
        &CaptureTarget::ProcessLoopback {
            executable: exe,
            include_tree: true,
        },
        40,
        Box::new(move |chunk: AudioChunk| {
            c.fetch_add(1, Ordering::Relaxed);
            let local = chunk.samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
            p.fetch_max((local * PEAK_SCALE) as u32, Ordering::Relaxed);
        }),
    )?;
    println!(
        "协商到的格式：{} Hz / {} 声道",
        format.sample_rate, format.channels
    );

    // 先让采集跑起来再出声，不然前几十毫秒会漏。
    std::thread::sleep(Duration::from_millis(300));
    play_tone(None)?;
    std::thread::sleep(Duration::from_millis(200));
    cap.stop();

    let heard = peak.load(Ordering::Relaxed) as f32 / PEAK_SCALE;
    println!(
        "收到 {} 块，峰值 {:.4}",
        chunks.load(Ordering::Relaxed),
        heard
    );
    if heard > 0.0 {
        println!("\x1b[32m进程环回通了：抓到了自己放的音。\x1b[0m");
    } else {
        println!("\x1b[31m峰值还是 0：进程环回没抓到自己的声音，这条路有问题。\x1b[0m");
    }
    Ok(())
}
