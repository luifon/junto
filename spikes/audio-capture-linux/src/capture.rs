use crate::common::{connect, monotonic_ns, roundtrip, stereo_f32_format_pod, AnyResult, Options, BYTES_PER_FRAME, CHANNELS, RATE};
use crate::graph::{ClientIdentity, Graph, PlaybackNode, Port, Selector, PLAYBACK_STREAM_CLASS};
use crate::tone_stream::{self, ToneSpec};
use pipewire as pw;
use pw::spa;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

const SILENCE_THRESHOLD: f32 = 1e-4;
const GAP_FILL_THRESHOLD_FRAMES: u64 = RATE as u64 / 10;
const ONSET_MIN_SILENCE_FRAMES: u64 = RATE as u64 / 5;

struct Block {
    callback_ns: u64,
    delay_ticks: i64,
    samples: Vec<f32>,
}

struct CaptureState {
    blocks: mpsc::Sender<Block>,
}

#[derive(Default)]
struct WriterStats {
    callbacks: u64,
    captured_frames: u64,
    padded_frames: u64,
    gaps: Vec<(f64, f64)>,
    block_frames: Vec<u32>,
    intervals_ns: Vec<u64>,
    delays: Vec<i64>,
    onsets: Vec<(u64, usize, usize, f64)>,
}

fn write_blocks(blocks: mpsc::Receiver<Block>, path: String, started_ns: u64) -> std::io::Result<WriterStats> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut stats = WriterStats::default();
    let mut written_frames: u64 = 0;
    let mut silent_run: u64 = ONSET_MIN_SILENCE_FRAMES;
    let mut previous_ns: Option<u64> = None;

    for block in blocks {
        let frames = (block.samples.len() / CHANNELS) as u64;
        let block_start_ns = block.callback_ns.saturating_sub(frames * 1_000_000_000 / RATE as u64);
        let expected_frames = (block_start_ns.saturating_sub(started_ns) as f64 * RATE as f64 / 1e9).round() as u64;
        if expected_frames > written_frames + GAP_FILL_THRESHOLD_FRAMES {
            let pad = expected_frames - written_frames;
            stats.gaps.push((written_frames as f64 / RATE as f64, pad as f64 / RATE as f64));
            for _ in 0..pad * CHANNELS as u64 {
                file.write_all(&0f32.to_le_bytes())?;
            }
            written_frames += pad;
            stats.padded_frames += pad;
            silent_run += pad;
        }
        for (frame_index, frame) in block.samples.chunks_exact(CHANNELS).enumerate() {
            let loud = frame.iter().any(|sample| sample.abs() > SILENCE_THRESHOLD);
            if loud {
                if silent_run >= ONSET_MIN_SILENCE_FRAMES {
                    let file_seconds = (written_frames + frame_index as u64) as f64 / RATE as f64;
                    stats.onsets.push((block.callback_ns, frame_index, frames as usize, file_seconds));
                }
                silent_run = 0;
            } else {
                silent_run += 1;
            }
        }
        for sample in &block.samples {
            file.write_all(&sample.to_le_bytes())?;
        }
        written_frames += frames;
        stats.callbacks += 1;
        stats.captured_frames += frames;
        stats.block_frames.push(frames as u32);
        stats.delays.push(block.delay_ticks);
        if let Some(previous) = previous_ns {
            stats.intervals_ns.push(block.callback_ns - previous);
        }
        previous_ns = Some(block.callback_ns);
    }
    file.flush()?;
    Ok(stats)
}

fn selector_from(options: &Options) -> AnyResult<Selector> {
    match options.required("mode")? {
        "system" => Ok(Selector::System { include_self: options.flag("include-self") }),
        "app" => {
            if let Some(process_id) = options.optional_number::<u32>("pid")? {
                Ok(Selector::ProcessId(process_id))
            } else if let Some(binary) = options.text("binary") {
                Ok(Selector::Binary(binary.to_string()))
            } else if let Some(id) = options.text("portal-app-id") {
                Ok(Selector::PortalApplicationId(id.to_string()))
            } else if let Some(name) = options.text("app-name") {
                Ok(Selector::ApplicationName(name.to_string()))
            } else {
                Err("app mode needs --pid, --binary, --portal-app-id or --app-name".into())
            }
        }
        other => Err(format!("unknown mode {other}").into()),
    }
}

fn number_property(properties: &spa::utils::dict::DictRef, key: &str) -> Option<u32> {
    properties.get(key).and_then(|text| text.parse().ok())
}

fn print_distribution(label: &str, values: &mut [f64], unit: &str) {
    if values.is_empty() {
        println!("STATS {label}: none");
        return;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    println!(
        "STATS {label}: min={:.3}{unit} median={:.3}{unit} mean={:.3}{unit} p99={:.3}{unit} max={:.3}{unit} n={}",
        values[0],
        values[values.len() / 2],
        mean,
        values[(values.len() * 99 / 100).min(values.len() - 1)],
        values[values.len() - 1],
        values.len()
    );
}

pub fn run(options: &Options) -> AnyResult<()> {
    let selector = selector_from(options)?;
    let seconds: f64 = options.number("seconds", 10.0)?;
    let out_path = options.required("out")?.to_string();
    let self_tone: Option<f64> = options.optional_number("self-tone")?;

    let (mainloop, _context, core) = connect("junto-spike-capture")?;
    let registry = core.get_registry_rc()?;
    let started_ns = monotonic_ns();
    println!("CAPTURE pid={} started_ns={started_ns}", std::process::id());

    let graph = Rc::new(RefCell::new(Graph::new(selector, options.flag("skip-link-groups"), started_ns)));
    let (block_sender, block_receiver) = mpsc::channel::<Block>();
    let writer = std::thread::spawn(move || write_blocks(block_receiver, out_path, started_ns));

    let mut properties = pw::properties::properties! {
        "media.type" => "Audio",
        "media.category" => "Capture",
        "media.role" => "Production",
        "node.name" => "junto-spike-capture",
        "node.autoconnect" => "false",
        "state.restore-props" => "false",
        "state.restore-target" => "false",
    };
    if let Some(latency) = options.text("latency") {
        properties.insert("node.latency", latency);
    }
    let stream = pw::stream::StreamRc::new(core.clone(), "junto-spike-capture", properties)?;
    let stream_listener = stream
        .add_local_listener_with_user_data(CaptureState { blocks: block_sender })
        .state_changed({
            let graph = graph.clone();
            let core = core.clone();
            move |stream, _, old, new| {
                let mut graph = graph.borrow_mut();
                graph.log(&format!("capture_stream_state {old:?} -> {new:?}"));
                let node_id = stream.node_id();
                if node_id != pw::constants::ID_ANY {
                    graph.set_capture_node(node_id);
                    graph.reconcile(&core);
                }
            }
        })
        .param_changed({
            let graph = graph.clone();
            move |_, _, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_ok() {
                    graph.borrow().log(&format!(
                        "capture_format format={:?} rate={} channels={} position={:?}",
                        info.format(),
                        info.rate(),
                        info.channels(),
                        &info.position()[..info.channels() as usize]
                    ));
                }
            }
        })
        .process(|stream, state| {
            let callback_ns = monotonic_ns();
            let delay_ticks = stream.time().map(|time| time.delay()).unwrap_or(i64::MIN);
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data = &mut datas[0];
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else { return };
            let end = (offset + size).min(bytes.len());
            let samples = bytes[offset.min(end)..end]
                .chunks_exact(BYTES_PER_FRAME / CHANNELS)
                .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
                .collect();
            let _ = state.blocks.send(Block { callback_ns, delay_ticks, samples });
        })
        .register()?;

    let format = stereo_f32_format_pod();
    let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("format pod")?];
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    let own_tone = match self_tone {
        Some(frequency) => Some(tone_stream::start(
            &core,
            ToneSpec {
                frequency,
                amplitude: options.number("amp", 0.2)?,
                lead_silence_frames: 0,
                tone_frames: None,
                target: options.required("self-target")?.to_string(),
                node_name: "junto-spike-own-playback".to_string(),
                latency: None,
            },
        )?),
        None => None,
    };

    let registry_listener = registry
        .add_listener_local()
        .global({
            let graph = graph.clone();
            let graph_for_nodes = graph.clone();
            let core = core.clone();
            let registry = registry.downgrade();
            move |global| {
                let Some(properties) = global.props else { return };
                match global.type_ {
                    ObjectType::Client => {
                        let Some(registry) = registry.upgrade() else { return };
                        let Ok(client) = registry.bind::<pw::client::Client, _>(global) else { return };
                        let client_id = global.id;
                        let listener = client
                            .add_listener_local()
                            .info({
                                let graph = graph.clone();
                                let core = core.clone();
                                move |info| {
                                    let Some(properties) = info.props() else { return };
                                    let identity = ClientIdentity {
                                        reported_process_id: number_property(properties, "application.process.id"),
                                        socket_process_id: number_property(properties, "pipewire.sec.pid"),
                                        binary: properties.get("application.process.binary").map(str::to_string),
                                        application_name: properties.get("application.name").map(str::to_string),
                                        api: properties.get("client.api").map(str::to_string),
                                        portal_application_id: properties.get("pipewire.access.portal.app_id").map(str::to_string),
                                    };
                                    let mut graph = graph.borrow_mut();
                                    graph.set_client_identity(client_id, identity);
                                    graph.reconcile(&core);
                                }
                            })
                            .register();
                        graph.borrow_mut().keep_client_proxy(client_id, client, listener);
                    }
                    ObjectType::Node => {
                        let mut graph = graph.borrow_mut();
                        if let Some(client_id) = number_property(properties, "client.id") {
                            graph.note_node_client(global.id, client_id);
                        }
                        if properties.get("media.class") != Some(PLAYBACK_STREAM_CLASS) {
                            graph.reconcile(&core);
                            return;
                        }
                        if graph.skips_link_groups() {
                            let Some(registry) = registry.upgrade() else { return };
                            let Ok(node) = registry.bind::<pw::node::Node, _>(global) else { return };
                            let node_id = global.id;
                            let listener = node
                                .add_listener_local()
                                .info({
                                    let graph = Rc::clone(&graph_for_nodes);
                                    let core = core.clone();
                                    move |info| {
                                        let link_group = info.props().and_then(|p| p.get("node.link-group")).map(str::to_string);
                                        let mut graph = graph.borrow_mut();
                                        graph.set_node_link_group(node_id, link_group);
                                        graph.reconcile(&core);
                                    }
                                })
                                .register();
                            graph.keep_node_proxy(node_id, node, listener);
                        }
                        graph.add_node(
                            global.id,
                            PlaybackNode {
                                serial: number_property(properties, "object.serial"),
                                client_id: number_property(properties, "client.id"),
                                name: properties.get("node.name").unwrap_or("").to_string(),
                                application_name: properties.get("application.name").map(str::to_string),
                            },
                        );
                        graph.reconcile(&core);
                    }
                    ObjectType::Port => {
                        let Some(node_id) = number_property(properties, "node.id") else { return };
                        let mut graph = graph.borrow_mut();
                        graph.add_port(
                            global.id,
                            Port {
                                node_id,
                                is_output: properties.get("port.direction") == Some("out"),
                                is_monitor: properties.get("port.monitor") == Some("true"),
                                channel: properties.get("audio.channel").unwrap_or("").to_string(),
                            },
                        );
                        graph.reconcile(&core);
                    }
                    ObjectType::Link => {
                        let graph = graph.borrow();
                        if graph.capture_node().is_some() && number_property(properties, "link.input.node") == graph.capture_node() {
                            graph.log(&format!(
                                "link_present link={} output_node={:?} output_port={:?} input_port={:?}",
                                global.id,
                                properties.get("link.output.node"),
                                properties.get("link.output.port"),
                                properties.get("link.input.port")
                            ));
                        }
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let graph = graph.clone();
            move |id| graph.borrow_mut().remove(id)
        })
        .register();

    let stop_timer = mainloop.loop_().add_timer({
        let mainloop = mainloop.downgrade();
        move |_| {
            if let Some(mainloop) = mainloop.upgrade() {
                mainloop.quit();
            }
        }
    });
    stop_timer.update_timer(Some(Duration::from_secs_f64(seconds)), None).into_result()?;
    mainloop.run();

    graph.borrow().log("capture_stopping");
    graph.borrow_mut().teardown();
    drop(stop_timer);
    drop(registry_listener);
    drop(own_tone);
    stream.disconnect()?;
    drop(stream_listener);
    drop(stream);
    roundtrip(&mainloop, &core)?;

    let stats = writer.join().map_err(|_| "writer thread panicked")??;
    println!(
        "STATS callbacks={} captured_frames={} padded_silence_frames={} total_seconds={:.3}",
        stats.callbacks,
        stats.captured_frames,
        stats.padded_frames,
        (stats.captured_frames + stats.padded_frames) as f64 / RATE as f64
    );
    print_distribution("frames_per_callback", &mut stats.block_frames.iter().map(|v| *v as f64).collect::<Vec<_>>(), "");
    print_distribution("callback_interval", &mut stats.intervals_ns.iter().map(|v| *v as f64 / 1e6).collect::<Vec<_>>(), "ms");
    print_distribution("stream_delay_ticks", &mut stats.delays.iter().map(|v| *v as f64).collect::<Vec<_>>(), "");
    for (start, length) in &stats.gaps {
        println!("STATS gap_filled_with_silence at={start:.3}s length={length:.3}s");
    }
    for (callback_ns, frame_in_block, block_frames, file_seconds) in &stats.onsets {
        println!("STATS onset callback_ns={callback_ns} frame_in_block={frame_in_block} block_frames={block_frames} file_time={file_seconds:.4}s");
    }
    Ok(())
}
