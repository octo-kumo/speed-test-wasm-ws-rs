use actix_files::Files;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_ws::Message;
use futures_util::stream::StreamExt;
use prost::Message as ProstMessage;
use proto::messages::{Request, Response};
use std::io::IsTerminal;
use std::{
    cmp::Ordering,
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Metrics {
    current_packets: u64,
    current_bytes: u64,
    packets_series: VecDeque<(f64, f64)>,
    bytes_series: VecDeque<(f64, f64)>,
    last_packets: u64,
    last_bytes: u64,
    last_packet_bytes: u64,
}

impl Metrics {
    const MAX_POINTS: usize = 240;
    fn push_sample(&mut self, x: f64) {
        self.packets_series
            .push_back((x, 2 * self.current_packets as f64));
        self.bytes_series
            .push_back((x, 2 * self.current_bytes as f64));

        self.last_packets = self.current_packets;
        self.last_bytes = self.current_bytes;

        self.current_packets = 0;
        self.current_bytes = 0;

        while self.packets_series.len() > Self::MAX_POINTS {
            self.packets_series.pop_front();
        }
        while self.bytes_series.len() > Self::MAX_POINTS {
            self.bytes_series.pop_front();
        }
    }
}

async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
    metrics: web::Data<Arc<Mutex<Metrics>>>,
) -> impl Responder {
    let (res, mut session, msg_stream) = match actix_ws::handle(&req, stream) {
        Ok(result) => result,
        Err(e) => {
            return HttpResponse::InternalServerError().body(e.to_string());
        }
    };
    let mut msg_stream = msg_stream.max_frame_size(1 * 1024 * 1024);
    let metrics = metrics.into_inner();

    actix_web::rt::spawn(async move {
        while let Some(msg) = msg_stream.next().await {
            match msg {
                Ok(Message::Binary(bin)) => {
                    if let Ok(mut m) = metrics.lock() {
                        m.current_packets = m.current_packets.saturating_add(1);
                        m.current_bytes = m.current_bytes.saturating_add(bin.len() as u64);
                        m.last_packet_bytes = bin.len() as u64;
                    }
                    if let Ok(req) = Request::decode(&*bin) {
                        let resp = Response {
                            result: format!("server received {} bytes", req.payload.len()),
                        };
                        let mut buf = Vec::new();
                        if resp.encode(&mut buf).is_ok() {
                            if let Err(e) = session.binary(buf).await {
                                eprintln!("Failed to send response: {}", e);
                                break;
                            }
                        } else {
                            eprintln!("Failed to encode Protobuf response");
                        }
                    } else {
                        eprintln!("Failed to decode Protobuf message");
                    }
                }
                Ok(Message::Ping(msg)) => {
                    if let Err(e) = session.pong(&msg).await {
                        eprintln!("Failed to send pong: {}", e);
                        break;
                    }
                }
                Ok(Message::Close(reason)) => {
                    let _ = session.close(reason).await;
                    break;
                }
                Err(e) => {
                    eprintln!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    res
}

use crossterm::{event, execute, terminal};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, Paragraph, Wrap},
    Terminal,
};

async fn run_tui(metrics: Arc<Mutex<Metrics>>, start: Instant) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(100);
    let window_secs: f64 = 60.0;
    loop {
        terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Length(3), Constraint::Min(10)])
                .split(size);

            let (lp, lb, now_s, last_packet_bytes) = {
                let m = metrics.lock().unwrap();
                let now_s = start.elapsed().as_secs_f64();
                (m.last_packets, m.last_bytes, now_s, m.last_packet_bytes)
            };

            let header = Paragraph::new(Line::from(vec![
                Span::styled(
                    "ws bench server  ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("• Interval: 0.5s  "),
                Span::raw(format!("• Now: {:>6.1}s  ", now_s)),
                Span::raw(format!("• Packets: {}  ", lp)),
                Span::raw(format!("• Bytes: {}  ", lb)),
                Span::raw(format!("• Last packet: {} bytes", last_packet_bytes)),
            ]))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(header, chunks[0]);

            let charts_area = chunks[1];
            let chart_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(charts_area);

            draw_series_chart(
                f,
                chart_chunks[0],
                &metrics,
                window_secs,
                start,
                SeriesKind::Packets,
            );
            draw_series_chart(
                f,
                chart_chunks[1],
                &metrics,
                window_secs,
                start,
                SeriesKind::Bytes,
            );
        })?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));

        if event::poll(timeout)? {
            if let event::Event::Key(key) = event::read()? {
                use crossterm::event::{KeyCode, KeyModifiers};
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => break,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    terminal::disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::LeaveAlternateScreen)?;
    Ok(())
}

#[derive(Copy, Clone)]
enum SeriesKind {
    Packets,
    Bytes,
}

fn draw_series_chart(
    f: &mut ratatui::Frame,
    area: Rect,
    metrics: &Arc<Mutex<Metrics>>,
    window_secs: f64,
    start: Instant,
    kind: SeriesKind,
) {
    let (points, title, y_label) = {
        let m = metrics.lock().unwrap();
        let (series, title, y_label) = match kind {
            SeriesKind::Packets => (&m.packets_series, "Packets", "pkts/s"),
            SeriesKind::Bytes => (&m.bytes_series, "Bytes", "bytes/s"),
        };
        (
            series.clone().into_iter().collect::<Vec<_>>(),
            title,
            y_label,
        )
    };

    let x_now = start.elapsed().as_secs_f64();
    let x_min = (x_now - window_secs).max(0.0);
    let x_max = x_now.max(1.0);

    let mut y_max = 1.0;
    for (x, y) in &points {
        if *x >= x_min && *x <= x_max {
            if *y > y_max {
                y_max = *y;
            }
        }
    }

    y_max = match y_max.partial_cmp(&1.0) {
        Some(Ordering::Less) | Some(Ordering::Equal) | None => 1.0,
        Some(Ordering::Greater) => (y_max * 1.2).ceil(),
    };

    let dataset = Dataset::default()
        .name(title)
        .marker(symbols::Marker::Dot)
        .graph_type(ratatui::widgets::GraphType::Line)
        .data(&points);

    let chart = Chart::new(vec![dataset])
        .block(Block::default().title(title).borders(Borders::ALL))
        .x_axis(
            Axis::default()
                .title(Span::styled(
                    "time (s)",
                    Style::default().add_modifier(Modifier::ITALIC),
                ))
                .bounds([x_min, x_max])
                .labels(vec![
                    Span::raw(format!("{:.0}", x_min)),
                    Span::raw(format!("{:.0}", (x_min + x_max) / 2.0)),
                    Span::raw(format!("{:.0}", x_max)),
                ]),
        )
        .y_axis(
            Axis::default()
                .title(Span::styled(
                    y_label,
                    Style::default().add_modifier(Modifier::ITALIC),
                ))
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", y_max / 2.0)),
                    Span::raw(format!("{:.0}", y_max)),
                ]),
        );

    f.render_widget(chart, area);
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let metrics = Arc::new(Mutex::new(Metrics::default()));
    let start = Instant::now();
    {
        let metrics_clone = Arc::clone(&metrics);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            let start = Instant::now();
            loop {
                interval.tick().await;
                if let Ok(mut m) = metrics_clone.lock() {
                    let x = start.elapsed().as_secs_f64();
                    m.push_sample(x);
                }
            }
        });
    }
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let port: u16 = port.parse().unwrap_or(8080);
    {
        let metrics_clone = Arc::clone(&metrics);
        tokio::spawn(async move {
            let server = HttpServer::new(move || {
                App::new()
                    .app_data(web::Data::new(Arc::clone(&metrics_clone)))
                    .route("/ws", web::get().to(ws_handler))
                    .service(Files::new("/", ".").index_file("index.html"))
            })
            .bind(("0.0.0.0", port))
            .expect(format!("Failed to bind to port {}", port).as_str())
            .run();
            println!("HTTP server running on port {}", port);
            if let Err(e) = server.await {
                eprintln!("HTTP server error: {}", e);
            }
        });
    }

    if std::io::stdout().is_terminal() {
        if let Err(e) = run_tui(metrics, start).await {
            eprintln!("TUI error: {}", e);
        }
    } else {
        println!("No TTY detected, running WebSocket server without TUI.");
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    Ok(())
}
