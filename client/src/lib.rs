use eframe::egui;
use egui_plot::Legend;
use egui_plot::{Line, Plot, PlotPoint, PlotPoints, Text};
use prost::Message as ProstMessage;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MIN_EXP: u32 = 6;
const MAX_EXP: u32 = 15;
const DURATION_SECS: u64 = 10;
const INITIAL_BURST_LIMIT: u64 = 10;
const MAX_BURST_LIMIT: u64 = 100000;
struct Bench {
    state: Arc<Mutex<AppState>>,
}

fn format_bytes(bytes: f64) -> String {
    let units = [
        "B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB", "RiB",
        "QiB", // imagine someone in 2100 actualling hitting these limits
    ];

    let abs = bytes.abs();
    if abs < 1.0 {
        return format!("{:.0}B", bytes);
    }
    let i = ((abs.log2() / 10.0).floor() as usize).min(units.len() - 1);
    let value = abs / (1_u64 << (i * 10)) as f64;

    let formatted = if (value.fract() - 0.0).abs() < std::f64::EPSILON {
        format!("{:.0}{}", value, units[i])
    } else {
        format!("{:.1}{}", value, units[i])
    };

    if bytes.is_sign_negative() {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

#[derive(Clone)]
struct RunSeries {
    size: usize,
    bytes_points: Vec<[f64; 2]>,
    packets_points: Vec<[f64; 2]>,
}

#[derive(Clone)]
struct AppState {
    response: String,
    bench_running: bool,
    bench_progress: f32,
    bench_status: String,
    bench_results: Vec<BenchResult>,
    live_bytes_per_s: f64,
    live_packets_per_s: f64,
    live_msgs_received_per_s: f64,
    live_size: usize,
    live_burst_limit: u64,
    live_ping_ms: f64,
    wait_for_reply: bool,
    stop_requested: bool,
    run_series: Vec<RunSeries>,
    show_detail_charts: bool,
}

#[derive(Clone)]
struct BenchResult {
    size: usize,
    bytes_per_s: f64,
    packets_per_s: f64,
    total_bytes: u64,
    total_packets: u64,
    total_time: f64,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            response: String::new(),
            bench_running: false,
            bench_progress: 0.0,
            bench_status: String::new(),
            bench_results: Vec::new(),
            live_bytes_per_s: 0.0,
            live_packets_per_s: 0.0,
            live_msgs_received_per_s: 0.0,
            live_size: 0,
            live_burst_limit: INITIAL_BURST_LIMIT,
            live_ping_ms: 0.0,
            wait_for_reply: true,
            stop_requested: false,
            run_series: Vec::new(),
            show_detail_charts: true,
        }
    }
}

impl eframe::App for Bench {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let s = self.state.lock().unwrap().clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("WebSocket + Protobuf Benchmark");
                ui.label("It sends protobuf messages of increasing size and measures throughput.");
                ui.label("Can be used as upload speed test as the server sends very little back.");
                ui.label("by yun");

                {
                    let s_snapshot = { self.state.lock().unwrap().clone() };
                    let mut wait_for_reply_local = {
                        let st = self.state.lock().unwrap();
                        st.wait_for_reply
                    };
                    let mut show_detail_local = {
                        let st = self.state.lock().unwrap();
                        st.show_detail_charts
                    };

                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut wait_for_reply_local, "Synchronous")
                            .on_hover_text("Wait for server reply before sending next message")
                            .changed()
                        {
                            let mut st = self.state.lock().unwrap();
                            st.wait_for_reply = wait_for_reply_local;
                        }

                        if ui
                            .checkbox(&mut show_detail_local, "Detail Chart")
                            .on_hover_text("Show detailed per-run time-series charts")
                            .changed()
                        {
                            let mut st = self.state.lock().unwrap();
                            st.show_detail_charts = show_detail_local;
                        }

                        ui.separator();
                        if ui
                            .add_enabled(
                                !s_snapshot.bench_running,
                                egui::widgets::Button::new("Start Benchmark"),
                            )
                            .clicked()
                        {
                            let state = self.state.clone();
                            start_benchmark(state);
                        }

                        if ui
                            .add_enabled(
                                s_snapshot.bench_running,
                                egui::widgets::Button::new("Stop"),
                            )
                            .clicked()
                        {
                            let mut st = self.state.lock().unwrap();
                            st.stop_requested = true;
                            st.bench_status = "Stop requested...".to_string();
                        }

                        if s_snapshot.bench_running {
                            ui.add(egui::widgets::Spinner::new());
                        }
                    });
                    ui.label(format!("Status: {}", s_snapshot.bench_status));
                }

                ui.add(
                    egui::widgets::ProgressBar::new(s.bench_progress)
                        .text(format!("{:.0}%", s.bench_progress * 100.0)),
                );

                ui.separator();

                let available_width = ui.available_width();
                let cutoff = 600.0_f32;

                if available_width > cutoff {
                    ui.horizontal(|ui| {
                        network_speed_plot(ui, &s, available_width / 2.0 - 8.0);
                        packet_speed_plot(ui, &s, available_width / 2.0 - 8.0);
                    });
                } else {
                    ui.vertical(|ui| {
                        network_speed_plot(ui, &s, available_width - 8.0);
                        packet_speed_plot(ui, &s, available_width - 8.0);
                    });
                }

                if s.show_detail_charts {
                    ui.separator();

                    if available_width > cutoff {
                        ui.horizontal(|ui| {
                            network_speed_over_time_plot(ui, &s, available_width / 2.0 - 8.0);
                            packet_speed_over_time_plot(ui, &s, available_width / 2.0 - 8.0);
                        });
                    } else {
                        ui.vertical(|ui| {
                            network_speed_over_time_plot(ui, &s, available_width - 8.0);
                            packet_speed_over_time_plot(ui, &s, available_width - 8.0);
                        });
                    }
                }

                ui.separator();

                ui.label("Live:");
                ui.horizontal(|ui| {
                    ui.label(format!("Size: {} B", s.live_size));
                    ui.label(format!("Bytes/s: {:.0}", s.live_bytes_per_s));
                    ui.label(format!("Msgs/s (sent): {:.0}", s.live_packets_per_s));
                    ui.label(format!("Msgs/s (recv): {:.0}", s.live_msgs_received_per_s));
                    if !s.wait_for_reply {
                        ui.label(format!("Burst limit: {}", s.live_burst_limit));
                    }
                    ui.label(format!("Ping: {:.0} ms", s.live_ping_ms));
                });

                ui.separator();

                ui.label("Results:");
                egui::Grid::new("results_grid")
                    .striped(true)
                    .min_col_width(40.0)
                    .show(ui, |ui| {
                        ui.heading("Packet Size");
                        ui.heading("Speed");
                        ui.heading("Msgs/s");
                        ui.heading("Bytes sent");
                        ui.heading("Packets sent");
                        ui.heading("Total time");
                        ui.end_row();
                        for r in &s.bench_results {
                            ui.label(format!("{} B", r.size));
                            ui.label(format!("{}/s", format_bytes(r.bytes_per_s)));
                            ui.label(format!("{:.0}", r.packets_per_s));
                            ui.label(format!("{}", format_bytes(r.total_bytes as f64)));
                            ui.label(format!("{}", r.total_packets));
                            ui.label(format!("{:.2}s", r.total_time));
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.label(format!("Last Response: {}", s.response));
            });
        });
    }
}

fn network_speed_plot(ui: &mut egui::Ui, s: &AppState, width: f32) {
    ui.vertical(|ui| {
        ui.label("Network Speed (bytes / s)");
        let points: Vec<[f64; 2]> = s
            .bench_results
            .iter()
            .map(|r| [(r.size as f64).log2(), r.bytes_per_s])
            .collect();

        let plot = Plot::new("bytes_per_s_plot").height(220.0).width(width);
        plot.x_axis_formatter(|x, _| format_bytes(x.value.exp2()))
            .y_axis_formatter(|y, _| format_bytes(y.value))
            .label_formatter(|_: &str, point| {
                format!(
                    "{}/s @ {}",
                    format_bytes(point.y),
                    format_bytes(point.x.exp2())
                )
            })
            .show(ui, |plot_ui| {
                if !points.is_empty() {
                    let pts = PlotPoints::from_iter(points);
                    plot_ui.line(Line::new("bytes_line".to_string(), pts));
                } else {
                    plot_ui.text(Text::new(
                        "no-data-label",
                        PlotPoint::new(0, 0),
                        "No data yet",
                    ));
                }
            });
    });
}

fn packet_speed_plot(ui: &mut egui::Ui, s: &AppState, width: f32) {
    ui.vertical(|ui| {
        ui.label("Packet Speed (msgs / s)");
        let points: Vec<[f64; 2]> = s
            .bench_results
            .iter()
            .map(|r| {
                let exponent = (r.size as f64).log2();
                [exponent, r.packets_per_s]
            })
            .collect();

        let plot = Plot::new("packets_per_s_plot").height(220.0).width(width);
        plot.x_axis_formatter(|x, _| format_bytes(x.value.exp2()))
            .label_formatter(|_: &str, point| {
                format!("{:.0} msg/s @ {}", point.y, format_bytes(point.x.exp2()))
            })
            .show(ui, |plot_ui| {
                if !points.is_empty() {
                    let pts = PlotPoints::from_iter(points);
                    plot_ui.line(Line::new("packets_line".to_string(), pts));
                } else {
                    plot_ui.text(Text::new(
                        "no-data-label",
                        PlotPoint::new(0, 0),
                        "No data yet",
                    ));
                }
            });
    });
}

fn network_speed_over_time_plot(ui: &mut egui::Ui, s: &AppState, width: f32) {
    ui.vertical(|ui| {
        ui.label("Network Speed Over Time (bytes / s) — by run");
        let plot = Plot::new("bytes_time_series").height(220.0).width(width);
        plot.legend(Legend::default().follow_insertion_order(true))
            .x_axis_formatter(|x, _| format!("{:.1}s", x.value))
            .y_axis_formatter(|y, _| format_bytes(y.value))
            .label_formatter(|label: &str, point| {
                format!("{}\n{}/s @ {:.2}", label, format_bytes(point.y), point.x)
            })
            .show(ui, |plot_ui| {
                if !s.run_series.is_empty() {
                    for run in s.run_series.iter() {
                        if !run.bytes_points.is_empty() {
                            let pts = PlotPoints::from_iter(run.bytes_points.clone());
                            plot_ui.line(Line::new(format_bytes(run.size as f64), pts));
                        }
                    }
                } else {
                    plot_ui.text(Text::new(
                        "no-data-label-time",
                        PlotPoint::new(0, 0),
                        "No time-series data yet",
                    ));
                }
            });
    });
}

fn packet_speed_over_time_plot(ui: &mut egui::Ui, s: &AppState, width: f32) {
    ui.vertical(|ui| {
        ui.label("Packet Speed Over Time (msg / s) — by run");

        let plot = Plot::new("packets_time_series").height(220.0).width(width);
        plot.legend(Legend::default().follow_insertion_order(true))
            .x_axis_formatter(|x, _| format!("{:.1}s", x.value))
            .label_formatter(|label: &str, point| {
                format!("{}\n{:.0} msg/s @ {:.2}", label, point.y, point.x)
            })
            .show(ui, |plot_ui| {
                if !s.run_series.is_empty() {
                    for run in s.run_series.iter() {
                        if !run.packets_points.is_empty() {
                            let pts = PlotPoints::from_iter(run.packets_points.clone());
                            plot_ui.line(Line::new(format_bytes(run.size as f64), pts));
                        }
                    }
                } else {
                    plot_ui.text(Text::new(
                        "no-data-label-time-pkts",
                        PlotPoint::new(0, 0),
                        "No time-series data yet",
                    ));
                }
            });
    });
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use futures_util::Stream;
    use futures_util::{sink::SinkExt, stream::StreamExt};
    use gloo_net::websocket::futures::WebSocket;
    use gloo_net::websocket::Message as WsMessage;
    use gloo_timers::future::sleep;
    use proto::messages::{Request, Response};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::spawn_local;
    use web_sys::console;
    use web_sys::HtmlCanvasElement;

    fn now_ms() -> f64 {
        web_sys::js_sys::Date::now()
    }

    fn set_status(state: &Arc<Mutex<AppState>>, s: impl Into<String>) {
        let mut st = state.lock().unwrap();
        st.bench_status = s.into();
    }

    fn process_received_bytes(bytes: Vec<u8>, state: &Arc<Mutex<AppState>>) {
        if let Ok(resp) = Response::decode(&*bytes) {
            let mut st = state.lock().unwrap();
            st.response = resp.result.clone();
        } else {
            console::log_1(&"failed to decode Response".into());
        }
    }

    fn compute_rates(
        bytes_sent: u64,
        packets_sent: u64,
        replies_received: u64,
        start_ms: f64,
    ) -> (f64, f64, f64, f64) {
        let elapsed_secs = ((now_ms() - start_ms) / 1000.0).max(1e-6);
        let bps = bytes_sent as f64 / elapsed_secs;
        let pps = packets_sent as f64 / elapsed_secs;
        let rps = replies_received as f64 / elapsed_secs;
        (bps, pps, rps, elapsed_secs)
    }

    fn update_live_metrics(state: &Arc<Mutex<AppState>>, bps: f64, pps: f64, rps: f64, prog: f32) {
        let mut st = state.lock().unwrap();
        st.live_bytes_per_s = bps;
        st.live_packets_per_s = pps;
        st.live_msgs_received_per_s = rps;

        st.bench_progress = prog;
    }

    async fn drain_until_zero<S, E>(
        rx: &mut S,
        state: &Arc<Mutex<AppState>>,
        outstanding: &mut u64,
        replies_received: &mut u64,
    ) where
        S: Stream<Item = Result<WsMessage, E>> + Unpin,
        E: std::fmt::Debug,
    {
        while *outstanding > 0 {
            if state.lock().unwrap().stop_requested {
                set_status(state, "Stopped by user");
                let mut st = state.lock().unwrap();
                st.bench_running = false;
                return;
            }

            match rx.next().await {
                Some(Ok(WsMessage::Bytes(bytes))) => {
                    process_received_bytes(bytes, state);
                    *replies_received = replies_received.saturating_add(1);
                    if *outstanding > 0 {
                        *outstanding = outstanding.saturating_sub(1);
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    console::log_1(&format!("read error: {:?}", e).into());
                    set_status(state, format!("Read error: {:?}", e));
                    return;
                }
                None => {
                    console::log_1(&"websocket stream ended".into());
                    set_status(state, "WebSocket closed");
                    return;
                }
            }
        }
    }
    pub fn start_benchmark(state: Arc<Mutex<AppState>>) {
        let wait_for_reply = {
            let st = state.lock().unwrap();
            st.wait_for_reply
        };

        spawn_local(async move {
            {
                let mut st = state.lock().unwrap();
                st.bench_running = true;
                st.bench_results.clear();
                st.bench_progress = 0.0;
                st.bench_status = "Opening WebSocket...".to_string();
                st.stop_requested = false;
                st.run_series.clear();
            }

            let window = web_sys::window().expect("no global `window` exists");
            let location = window.location();
            let host = match location.host() {
                Ok(h) => h,
                Err(_) => {
                    set_status(&state, "Failed to get host");
                    let mut st = state.lock().unwrap();
                    st.bench_running = false;
                    return;
                }
            };
            let host = host
                .trim_start_matches("wss://")
                .trim_start_matches("ws://")
                .trim_end_matches('/');

            let ws_url = format!("wss://{}/ws", host);
            let socket = match WebSocket::open(ws_url.as_str()) {
                Ok(s) => s,
                Err(e) => {
                    let wss_url = format!("ws://{}/ws", host);
                    match WebSocket::open(wss_url.as_str()) {
                        Ok(s2) => s2,
                        Err(e2) => {
                            set_status(
                                &state,
                                format!(
                                    "Failed to open websocket: ws error: {:?}; wss error: {:?}",
                                    e, e2
                                ),
                            );
                            let mut st = state.lock().unwrap();
                            st.bench_running = false;
                            return;
                        }
                    }
                }
            };
            let (mut tx, mut rx) = socket.split();

            set_status(&state, "Starting benchmark...");

            let mut burst_limit: u64 = INITIAL_BURST_LIMIT;
            let ping_req = Request {
                action: "ping".to_string(),
                payload: "p".to_string(),
            };
            let mut ping_buf = Vec::new();
            if ping_req.encode(&mut ping_buf).is_ok() {
                if tx.send(WsMessage::Bytes(ping_buf.clone())).await.is_ok() {
                    let ping_start = now_ms();
                    if let Some(Ok(WsMessage::Bytes(bytes))) = rx.next().await {
                        let ping_ms = (now_ms() - ping_start).max(0.0);
                        {
                            let mut st = state.lock().unwrap();
                            st.live_ping_ms = ping_ms;
                        }
                        process_received_bytes(bytes, &state);
                    }
                }
            }
            {
                let mut st = state.lock().unwrap();
                st.live_burst_limit = burst_limit;
            }

            let mut last_outstanding: u64 = 0;
            let mut consecutive_zero_samples: usize = 0;

            let sizes: Vec<usize> = (MIN_EXP..=MAX_EXP).map(|e| 1usize << e).collect();
            let total_steps = sizes.len();
            let mut step = 0usize;

            for &size in &sizes {
                if state.lock().unwrap().stop_requested {
                    set_status(&state, "Stopped by user");
                    let mut st = state.lock().unwrap();
                    st.bench_running = false;
                    return;
                }

                {
                    let mut st = state.lock().unwrap();
                    st.live_size = size;
                    st.bench_status = format!("Benchmarking {} bytes...", size);
                    st.bench_progress = step as f32 / (total_steps as f32);
                    st.live_bytes_per_s = 0.0;
                    st.live_packets_per_s = 0.0;
                    st.live_msgs_received_per_s = 0.0;
                }

                let payload_string = "A".repeat(size);
                let req_proto = Request {
                    action: "bench".to_string(),
                    payload: payload_string,
                };
                let mut buf = Vec::new();
                if req_proto.encode(&mut buf).is_err() {
                    console::log_1(&"failed to encode bench request".into());
                    set_status(&state, "Encode failed");
                    break;
                }

                let mut bytes_sent: u64 = 0;
                let mut packets_sent: u64 = 0;
                let mut replies_received: u64 = 0;
                let mut outstanding: u64 = 0;

                let start_ms = now_ms();
                let duration_ms = (DURATION_SECS * 1000) as f64;

                let mut last_sample_elapsed: f64 = 0.0;
                let mut current_run = RunSeries {
                    size,
                    bytes_points: Vec::new(),
                    packets_points: Vec::new(),
                };
                let mut last_received_time = 0.0;
                loop {
                    if state.lock().unwrap().stop_requested {
                        set_status(&state, "Stopped by user");
                        let mut st = state.lock().unwrap();
                        st.bench_running = false;
                        return;
                    }

                    let elapsed = now_ms() - start_ms;
                    if elapsed >= duration_ms {
                        break;
                    }

                    match tx.send(WsMessage::Bytes(buf.clone())).await {
                        Ok(_) => {
                            bytes_sent = bytes_sent.saturating_add(buf.len() as u64);
                            packets_sent = packets_sent.saturating_add(1);
                            outstanding = outstanding.saturating_add(1);
                        }
                        Err(e) => {
                            console::log_1(&format!("send error: {:?}", e).into());
                            set_status(&state, format!("Send error: {:?}", e));
                            break;
                        }
                    }
                    let last_receive_gap = elapsed - last_received_time;
                    if wait_for_reply {
                        match rx.next().await {
                            Some(Ok(WsMessage::Bytes(bytes))) => {
                                process_received_bytes(bytes, &state);
                                replies_received = replies_received.saturating_add(1);
                                outstanding = outstanding.saturating_sub(1);
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                console::log_1(&format!("read error: {:?}", e).into());
                                set_status(&state, format!("Read error: {:?}", e));
                                break;
                            }
                            None => {
                                console::log_1(&"websocket stream ended".into());
                                set_status(&state, "WebSocket closed");
                                break;
                            }
                        }
                        last_received_time = elapsed;
                    } else {
                        if outstanding >= burst_limit {
                            drain_until_zero(
                                &mut rx,
                                &state,
                                &mut outstanding,
                                &mut replies_received,
                            )
                            .await;
                            last_received_time = elapsed;
                        }
                    }

                    let (bps, pps, rps, elapsed_secs) =
                        compute_rates(bytes_sent, packets_sent, replies_received, start_ms);
                    if (elapsed_secs - last_sample_elapsed) >= 0.1 && replies_received > 0 {
                        current_run.bytes_points.push([elapsed_secs, bps]);
                        current_run.packets_points.push([elapsed_secs, pps]);
                        if outstanding == 0 {
                            consecutive_zero_samples = consecutive_zero_samples.saturating_add(1);
                        } else {
                            consecutive_zero_samples = 0;
                        }

                        if (outstanding > last_outstanding
                            && outstanding > (burst_limit / 3).max(1))
                            || last_receive_gap > 500.0
                        {
                            let new_limit = ((burst_limit as f64) * 0.75).max(1.0) as u64;
                            if new_limit < burst_limit {
                                burst_limit = new_limit;
                            }
                        } else if consecutive_zero_samples >= 4
                            && burst_limit < MAX_BURST_LIMIT
                            && last_receive_gap < 300.0
                        {
                            burst_limit = ((burst_limit as f64) * 1.2).ceil() as u64;
                            if burst_limit > MAX_BURST_LIMIT {
                                burst_limit = MAX_BURST_LIMIT;
                            }
                        }
                        {
                            let mut st = state.lock().unwrap();
                            st.live_burst_limit = burst_limit;
                        }

                        last_outstanding = outstanding;
                        last_sample_elapsed = elapsed_secs;
                        update_live_metrics(
                            &state,
                            bps,
                            pps,
                            rps,
                            ((step as f32) + (elapsed / duration_ms).clamp(0.0, 1.0) as f32)
                                / (total_steps as f32),
                        );
                    }
                }
                if outstanding > 0 {
                    drain_until_zero(&mut rx, &state, &mut outstanding, &mut replies_received)
                        .await;
                }

                let total_bytes = bytes_sent;
                let total_packets = packets_sent;
                let (_, _, rps, elapsed_secs) =
                    compute_rates(total_bytes, total_packets, replies_received, start_ms);
                let bps = total_bytes as f64 / elapsed_secs;
                let pps = total_packets as f64 / elapsed_secs;

                {
                    let mut st = state.lock().unwrap();
                    st.bench_results.push(BenchResult {
                        size,
                        bytes_per_s: bps,
                        packets_per_s: pps,
                        total_bytes,
                        total_packets,
                        total_time: elapsed_secs,
                    });

                    st.run_series.push(current_run.clone());

                    st.bench_status = format!(
                        "Completed {} B: {:.0} B/s, {:.0} msg/s (sent), {:.0} msg/s (recv)",
                        size, bps, pps, rps
                    );
                    st.live_bytes_per_s = bps;
                    st.live_packets_per_s = pps;
                    st.live_msgs_received_per_s = rps;
                }
                step += 1;
                sleep(Duration::from_millis(1000)).await;
                burst_limit = ((burst_limit as f64) * 0.7) as u64;
            }

            {
                let mut st = state.lock().unwrap();
                st.bench_running = false;
                st.bench_progress = 1.0;
                st.bench_status = "Finished".to_string();
                st.stop_requested = false;
            }
        });
    }

    #[wasm_bindgen(start)]
    pub fn wasm_main() -> Result<(), wasm_bindgen::JsValue> {
        console_error_panic_hook::set_once();
        log::set_max_level(log::LevelFilter::Debug);
        let state = Arc::new(Mutex::new(AppState::default()));
        let app = Bench {
            state: state.clone(),
        };

        let document = web_sys::window().unwrap().document().unwrap();
        let canvas = document
            .get_element_by_id("canvas")
            .unwrap()
            .dyn_into::<HtmlCanvasElement>()?;
        if canvas.get_context("webgl")?.is_none() {
            document.body().unwrap().set_inner_html(
                "<div style='text-align:center; font-family:monospace;'>WebGL not supported</div>",
            );
            return Ok(());
        }
        let runner = eframe::WebRunner::new();
        eframe::WebLogger::init(log::LevelFilter::Debug).ok();
        spawn_local(async move {
            let _ = runner
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(move |_cc| Ok(Box::new(app))),
                )
                .await;
        });

        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
use wasm_impl::start_benchmark;
#[cfg(target_arch = "wasm32")]
use wasm_impl::wasm_main as main;

/**
 * Native implementation for non-WASM targets.
 * will need another websocket library.
 */

#[cfg(not(target_arch = "wasm32"))]
fn start_benchmark(state: Arc<Mutex<AppState>>) {
    std::thread::spawn(move || {
        {
            let mut st = state.lock().unwrap();
            st.bench_running = true;
            st.bench_progress = 0.0;
            st.bench_status = "not implemented".to_string();
            st.stop_requested = false;
            st.run_series.clear();
        }
        let mut elapsed = 0u64;
        while elapsed < 500 {
            if state.lock().unwrap().stop_requested {
                let mut st = state.lock().unwrap();
                st.bench_status = "Stopped by user".to_string();
                st.bench_running = false;
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            elapsed += 50;
        }

        {
            let mut st = state.lock().unwrap();
            st.bench_running = false;
            st.bench_progress = 1.0;
            st.bench_status = "Finished (native stub)".to_string();
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let native_options = eframe::NativeOptions::default();
    let state = Arc::new(Mutex::new(AppState::default()));
    let app = Bench {
        state: state.clone(),
    };

    eframe::run_native(
        "ws + proto benchmark",
        native_options,
        Box::new(|_cc| Box::new(app)),
    );
}
