use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    app::{AppState, DiskSnapshot, HostSnapshot, Mode},
    theme,
};

const MAX_DISK_MOUNT_LABEL_WIDTH: usize = 12;
const MIN_TERMINAL_WIDTH: u16 = 80;
const MIN_TERMINAL_HEIGHT: u16 = 24;
const CARD_BORDER_ROWS: u16 = 2;
const CARD_FIXED_CONTENT_ROWS: usize = 6;
const MAX_DISK_ROWS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSizeRequirement {
    width: u16,
    height: u16,
}

pub struct Dashboard<'a> {
    pub state: &'a AppState,
}

pub fn draw(frame: &mut Frame<'_>, dashboard: &Dashboard<'_>) {
    let area = frame.area();
    let root = Block::new().style(Style::default().bg(theme::BG).fg(theme::TEXT));
    frame.render_widget(root, area);

    let requirement = minimum_terminal_size();
    if area.width < requirement.width || area.height < requirement.height {
        draw_terminal_too_small(frame, area, requirement);
        return;
    }

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(18)])
        .margin(1)
        .split(area);

    draw_header(frame, vertical[0], dashboard.state);
    draw_host_grid(frame, vertical[1], dashboard.state);
}

fn minimum_terminal_size() -> TerminalSizeRequirement {
    TerminalSizeRequirement {
        width: MIN_TERMINAL_WIDTH,
        height: MIN_TERMINAL_HEIGHT,
    }
}

fn draw_terminal_too_small(
    frame: &mut Frame<'_>,
    area: Rect,
    requirement: TerminalSizeRequirement,
) {
    let width_ok = area.width >= requirement.width;
    let height_ok = area.height >= requirement.height;
    let width_style = Style::default().fg(if width_ok {
        ratatui::style::Color::Green
    } else {
        ratatui::style::Color::Red
    });
    let height_style = Style::default().fg(if height_ok {
        ratatui::style::Color::Green
    } else {
        ratatui::style::Color::Red
    });

    let message = vec![
        Line::from(Span::styled(
            "Terminal size too small:",
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("Width = "),
            Span::styled(area.width.to_string(), width_style),
            Span::raw(" Height = "),
            Span::styled(area.height.to_string(), height_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Needed for current config:",
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "Width = {} Height = {}",
            requirement.width, requirement.height
        )),
        Line::from(""),
        Line::from(Span::styled(
            "q / ctrl-c / esc exits",
            Style::default().fg(theme::MUTED),
        )),
    ];

    let popup = centered_rect(
        area,
        area.width.min(40),
        area.height.min(message.len() as u16),
    );
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().bg(theme::BG).fg(theme::TEXT))
            .alignment(Alignment::Left),
        popup,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let summary = Line::from(vec![
        Span::styled("▣ ", Style::default().fg(theme::ACCENT)),
        Span::styled(
            &state.title,
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            header_detail_text(state.hosts.len(), state.refresh_interval_ms),
            Style::default().fg(theme::MUTED),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(vec![summary])
            .block(panel_block(" Overview "))
            .alignment(Alignment::Left),
        area,
    );
}

fn header_detail_text(host_count: usize, refresh_interval_ms: u64) -> String {
    format!(
        "{} hosts · refresh {} · +/- 100ms · q/ctrl-c/esc exits",
        host_count,
        format_refresh_ms(refresh_interval_ms)
    )
}

fn draw_host_grid(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if state.hosts.is_empty() {
        frame.render_widget(
            Paragraph::new("No enabled hosts configured").block(panel_block(" Hosts ")),
            area,
        );
        return;
    }

    let block = panel_block(" Hosts ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let columns = grid_columns(inner.width, state.hosts.len());
    let row_infos = row_layout_infos(&state.hosts, columns);
    let column_disk_mount_widths = column_disk_mount_widths(&state.hosts, columns);
    let rows = disk_priority_row_areas(inner, &row_infos);

    for (row_info, row_area) in row_infos.iter().zip(rows.iter()) {
        let row_hosts = &state.hosts[row_info.start..row_info.end];
        let col_constraints = vec![Constraint::Ratio(1, row_hosts.len() as u32); row_hosts.len()];
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for (col_index, (host, col)) in row_hosts.iter().zip(cols.iter()).enumerate() {
            let layout = card_content_layout(
                col.height.saturating_sub(CARD_BORDER_ROWS),
                row_info.disk_rows,
            );
            let mount_width = column_disk_mount_widths
                .get(col_index)
                .copied()
                .unwrap_or(1);
            draw_host_card(frame, *col, host, layout, mount_width);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowLayoutInfo {
    start: usize,
    end: usize,
    disk_rows: usize,
}

fn row_layout_infos(hosts: &[HostSnapshot], columns: usize) -> Vec<RowLayoutInfo> {
    hosts
        .chunks(columns)
        .enumerate()
        .map(|(row_index, row_hosts)| {
            let start = row_index * columns;
            let end = start + row_hosts.len();
            let disk_rows = row_hosts
                .iter()
                .filter(|host| host.status.badge() != "UNAVAILABLE")
                .map(|host| host.disks.len().min(MAX_DISK_ROWS))
                .max()
                .unwrap_or(0);
            RowLayoutInfo {
                start,
                end,
                disk_rows,
            }
        })
        .collect()
}

fn column_disk_mount_widths(hosts: &[HostSnapshot], columns: usize) -> Vec<usize> {
    let mut widths = vec![1; columns.max(1)];
    for (index, host) in hosts.iter().enumerate() {
        if host.status.badge() == "UNAVAILABLE" {
            continue;
        }
        let column = index % columns.max(1);
        for disk in host.disks.iter().take(MAX_DISK_ROWS) {
            widths[column] = widths[column]
                .max(short_mount(&disk.mount).chars().count())
                .min(MAX_DISK_MOUNT_LABEL_WIDTH);
        }
    }
    widths
}

fn disk_priority_row_areas(area: Rect, row_infos: &[RowLayoutInfo]) -> Vec<Rect> {
    let heights = disk_priority_row_heights(area.height, row_infos);
    let mut y = area.y;
    heights
        .into_iter()
        .map(|height| {
            let rect = Rect::new(area.x, y, area.width, height);
            y = y.saturating_add(height);
            rect
        })
        .collect()
}

fn disk_priority_row_heights(total_height: u16, row_infos: &[RowLayoutInfo]) -> Vec<u16> {
    if row_infos.is_empty() {
        return Vec::new();
    }

    let mut heights = row_infos
        .iter()
        .map(|row| preferred_row_height(row.disk_rows))
        .collect::<Vec<_>>();
    let preferred_total = heights.iter().sum::<u16>();

    if preferred_total < total_height {
        distribute_extra_height(&mut heights, total_height - preferred_total);
    } else if preferred_total > total_height {
        shrink_row_heights(&mut heights, preferred_total - total_height);
    }

    let used = heights.iter().sum::<u16>();
    if let Some(last) = heights.last_mut() {
        *last = last.saturating_add(total_height.saturating_sub(used));
    }
    heights
}

fn preferred_row_height(disk_rows: usize) -> u16 {
    CARD_BORDER_ROWS + CARD_FIXED_CONTENT_ROWS as u16 + disk_rows.min(MAX_DISK_ROWS) as u16
}

fn distribute_extra_height(heights: &mut [u16], mut extra: u16) {
    let mut index = 0usize;
    while extra > 0 && !heights.is_empty() {
        heights[index] = heights[index].saturating_add(1);
        extra -= 1;
        index = (index + 1) % heights.len();
    }
}

fn shrink_row_heights(heights: &mut [u16], mut excess: u16) {
    const MIN_ROW_HEIGHT: u16 = CARD_BORDER_ROWS + CARD_FIXED_CONTENT_ROWS as u16;
    while excess > 0 {
        let Some((index, _)) = heights
            .iter()
            .enumerate()
            .filter(|(_, height)| **height > MIN_ROW_HEIGHT)
            .max_by_key(|(_, height)| **height)
        else {
            break;
        };
        heights[index] -= 1;
        excess -= 1;
    }
}

fn grid_columns(width: u16, host_count: usize) -> usize {
    let columns = if width >= 180 {
        3
    } else if width >= 88 {
        2
    } else {
        1
    };
    columns.min(host_count.max(1))
}

fn draw_host_card(
    frame: &mut Frame<'_>,
    area: Rect,
    host: &HostSnapshot,
    layout: CardContentLayout,
    disk_mount_width: usize,
) {
    let block = Block::default()
        .title(format!(" {} {} ", host.name, host.status.badge()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(host.status.color()))
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(vec![Span::styled(
            &host.role,
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::raw(host.hostname.as_deref().unwrap_or("-")),
            Span::styled(" · ", Style::default().fg(theme::MUTED)),
            Span::raw(short_kernel(host.kernel.as_deref())),
            Span::styled(" · ", Style::default().fg(theme::MUTED)),
            Span::raw(format_uptime(host.uptime_seconds)),
        ]),
    ];
    let available = host.status.badge() != "UNAVAILABLE";
    let history_width = history_graph_width(inner.width);
    lines.push(cpu_section_line(host, inner.width));
    if available {
        lines.extend(history_graph_lines(
            &host.cpu_history,
            theme::CPU,
            layout.cpu_graph_rows,
            history_width,
        ));
    } else {
        lines.extend(blank_lines(layout.cpu_graph_rows));
    }

    lines.push(section_line(
        "ram",
        &ram_detail(host),
        theme::RAM,
        inner.width,
    ));
    if available {
        lines.extend(history_graph_lines(
            &host.ram_history,
            theme::RAM,
            layout.ram_graph_rows,
            history_width,
        ));
    } else {
        lines.extend(blank_lines(layout.ram_graph_rows));
    }

    lines.push(net_line(host, inner.width));
    if available {
        lines.extend(history_graph_lines(
            &host.net_history,
            theme::NETWORK,
            layout.net_graph_rows,
            history_width,
        ));
    } else {
        lines.extend(blank_lines(layout.net_graph_rows));
    }

    if !available || host.disks.is_empty() {
        lines.push(disk_no_data_line(inner.width));
        lines.extend(blank_lines(layout.disk_rows));
    } else {
        lines.push(disk_section_line(host, inner.width));
        let rendered_disk_rows = host.disks.len().min(layout.disk_rows);
        lines.extend(disk_lines(
            &host.disks,
            layout.disk_rows,
            inner.width,
            disk_mount_width,
        ));
        lines.extend(blank_lines(
            layout.disk_rows.saturating_sub(rendered_disk_rows),
        ));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme::TEXT)),
        inner,
    );
}

fn section_line(
    label: &'static str,
    parts: &[String],
    color: ratatui::style::Color,
    inner_width: u16,
) -> Line<'static> {
    let parts = parts
        .iter()
        .map(|part| SectionDetail::new(part.clone(), theme::TEXT))
        .collect::<Vec<_>>();
    section_line_with_details(label, &parts, color, inner_width)
}

#[derive(Debug, Clone)]
struct SectionDetail {
    text: String,
    color: ratatui::style::Color,
}

impl SectionDetail {
    fn new(text: String, color: ratatui::style::Color) -> Self {
        Self { text, color }
    }
}

fn section_line_with_details(
    label: &'static str,
    parts: &[SectionDetail],
    color: ratatui::style::Color,
    inner_width: u16,
) -> Line<'static> {
    let title = label.to_string();
    let prefix_len = 1 + title.chars().count();
    let separator_len = parts.len();
    let detail_len = parts
        .iter()
        .map(|part| part.text.chars().count())
        .sum::<usize>()
        + separator_len;
    let rule_len = usize::from(inner_width)
        .saturating_sub(prefix_len + detail_len)
        .max(1);

    let mut spans = vec![
        Span::styled("─", Style::default().fg(theme::BORDER)),
        Span::styled(
            title,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("─".repeat(rule_len), Style::default().fg(theme::BORDER)),
    ];

    for (idx, part) in parts.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("─", Style::default().fg(theme::BORDER)));
        }
        spans.push(Span::styled(
            part.text.clone(),
            Style::default().fg(part.color),
        ));
    }
    spans.push(Span::styled("─", Style::default().fg(theme::BORDER)));

    Line::from(spans)
}

fn cpu_section_line(host: &HostSnapshot, inner_width: u16) -> Line<'static> {
    let details = cpu_detail_styled(host);
    section_line_with_details("cpu", &details, theme::CPU, inner_width)
}

fn history_graph_lines(
    history: &[u16],
    color: ratatui::style::Color,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    vertical_braille_graph(history, rows, width)
        .into_iter()
        .map(|graph| graph_line(&graph, color))
        .collect::<Vec<_>>()
}

fn blank_lines(count: usize) -> Vec<Line<'static>> {
    std::iter::repeat_with(|| Line::from(""))
        .take(count)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct CardContentLayout {
    cpu_graph_rows: usize,
    ram_graph_rows: usize,
    net_graph_rows: usize,
    disk_rows: usize,
}

fn card_content_layout(inner_height: u16, row_disk_count: usize) -> CardContentLayout {
    let height = usize::from(inner_height);
    let disk_rows = row_disk_count
        .min(MAX_DISK_ROWS)
        .min(height.saturating_sub(CARD_FIXED_CONTENT_ROWS));
    let graph_rows = height.saturating_sub(CARD_FIXED_CONTENT_ROWS + disk_rows);

    let base_graph_rows = graph_rows / 3;
    let extra_graph_rows = graph_rows % 3;
    let cpu_graph_rows = base_graph_rows + usize::from(extra_graph_rows > 0);
    let ram_graph_rows = base_graph_rows + usize::from(extra_graph_rows > 1);
    let net_graph_rows = base_graph_rows;

    CardContentLayout {
        cpu_graph_rows,
        ram_graph_rows,
        net_graph_rows,
        disk_rows,
    }
}

fn history_graph_width(inner_width: u16) -> usize {
    usize::from(inner_width).max(18)
}

#[derive(Debug, Clone, Copy)]
struct GraphCell {
    glyph: char,
    active: bool,
}

fn graph_line(cells: &[GraphCell], color: ratatui::style::Color) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current_active = None;
    let mut text = String::new();

    for cell in cells {
        if current_active.is_some_and(|active| active != cell.active) {
            push_graph_span(&mut spans, std::mem::take(&mut text), current_active, color);
        }
        current_active = Some(cell.active);
        text.push(cell.glyph);
    }
    push_graph_span(&mut spans, text, current_active, color);

    Line::from(spans)
}

fn push_graph_span(
    spans: &mut Vec<Span<'static>>,
    text: String,
    active: Option<bool>,
    color: ratatui::style::Color,
) {
    if text.is_empty() {
        return;
    }
    let graph_color = if active.unwrap_or(false) {
        color
    } else {
        theme::BORDER
    };
    spans.push(Span::styled(text, Style::default().fg(graph_color)));
}

fn vertical_braille_graph(history: &[u16], rows: usize, width: usize) -> Vec<Vec<GraphCell>> {
    const LEFT_DOTS: [u8; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT_DOTS: [u8; 4] = [0x80, 0x20, 0x10, 0x08];

    if rows == 0 || width == 0 {
        return Vec::new();
    }

    let total_pixels = rows * 4;
    let max_points = width * 2;
    let source = if history.is_empty() {
        &[0][..]
    } else {
        history
    };
    let points = resampled_history_points(source, max_points);
    let mut masks = vec![vec![0u8; width]; rows];
    let Some((low, high)) = graph_range(&points) else {
        return masks
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|_| GraphCell {
                        glyph: ' ',
                        active: false,
                    })
                    .collect()
            })
            .collect();
    };
    let range = usize::from(high.saturating_sub(low).max(1));

    for (idx, percent) in points.iter().enumerate() {
        let capped = (*percent).clamp(low, high);
        let x = idx / 2;
        if x >= width {
            continue;
        }
        let offset = usize::from(capped.saturating_sub(low));
        let y = ((offset * (total_pixels - 1)) + (range / 2)) / range;
        let dots = if idx % 2 == 0 { LEFT_DOTS } else { RIGHT_DOTS };
        for filled_y in 0..=y {
            let row = rows - 1 - (filled_y / 4);
            let sub_row = filled_y % 4;
            masks[row][x] |= dots[sub_row];
        }
    }

    masks
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|mask| {
                    let glyph = if mask == 0 {
                        ' '
                    } else {
                        char::from_u32(0x2800 + u32::from(mask)).unwrap_or(' ')
                    };
                    GraphCell {
                        glyph,
                        active: mask != 0,
                    }
                })
                .collect()
        })
        .collect()
}

fn resampled_history_points(source: &[u16], target_len: usize) -> Vec<u16> {
    if target_len == 0 {
        return Vec::new();
    }
    if source.is_empty() {
        return vec![0; target_len];
    }
    if source.len() == 1 {
        return vec![source[0]; target_len];
    }

    let last = source.len() - 1;
    let target_last = target_len.saturating_sub(1).max(1);
    (0..target_len)
        .map(|idx| {
            let source_idx = (idx * last + target_last / 2) / target_last;
            source[source_idx]
        })
        .collect()
}

fn graph_range(points: &[u16]) -> Option<(u16, u16)> {
    let max = points.iter().copied().max()?.min(100);
    if max == 0 {
        return None;
    }
    let min = points.iter().copied().min().unwrap_or(max).min(100);
    let span = max.saturating_sub(min);
    if span >= 14 {
        return Some((min, max));
    }

    let center = (u32::from(min) + u32::from(max)) / 2;
    let half_span = 7u32;
    let mut low = center.saturating_sub(half_span) as u16;
    let mut high = (center + half_span).min(100) as u16;
    if high.saturating_sub(low) < 14 {
        low = high.saturating_sub(14);
        high = (low + 14).min(100);
    }
    Some((low, high))
}

fn progress_bar(percent: u16, width: usize) -> String {
    let filled = ((usize::from(percent.min(100)) * width) + 50) / 100;
    let mut bar = String::with_capacity(width + 2);
    bar.push('▕');
    bar.extend(std::iter::repeat_n('█', filled));
    bar.extend(std::iter::repeat_n('░', width.saturating_sub(filled)));
    bar.push('▏');
    bar
}

fn load_text(host: &HostSnapshot) -> String {
    match (host.load_1m, host.load_5m, host.load_15m) {
        (Some(a), Some(b), Some(c)) => format!("{a:.2} {b:.2} {c:.2}"),
        (Some(a), _, _) => format!("{a:.2}"),
        _ => "-".to_string(),
    }
}

#[cfg(test)]
fn cpu_detail(host: &HostSnapshot) -> Vec<String> {
    cpu_detail_styled(host)
        .into_iter()
        .map(|detail| detail.text)
        .collect()
}

fn cpu_detail_styled(host: &HostSnapshot) -> Vec<SectionDetail> {
    if host.load_1m.is_none() && host.cpu_cores.is_none() && host.cpu_temperature_celsius.is_none()
    {
        return vec![SectionDetail::new("no data".to_string(), theme::TEXT)];
    }

    let mut parts = Vec::new();
    if let Some(temp) = host.cpu_temperature_celsius {
        parts.push(SectionDetail::new(
            temperature_text(Some(temp)),
            temperature_color(temp),
        ));
    }
    parts.push(SectionDetail::new(
        format!("{}%", host.cpu_percent),
        theme::TEXT,
    ));
    parts
}

fn temperature_text(temperature_celsius: Option<f32>) -> String {
    temperature_celsius
        .map(|temp| format!("{temp:.1}°C"))
        .unwrap_or_else(|| "-".to_string())
}

fn temperature_color(temperature_celsius: f32) -> ratatui::style::Color {
    if temperature_celsius >= 85.0 {
        ratatui::style::Color::Red
    } else if temperature_celsius >= 70.0 {
        ratatui::style::Color::Yellow
    } else {
        theme::TEXT
    }
}

fn ram_detail(host: &HostSnapshot) -> Vec<String> {
    if host.ram_used_kib.is_none() && host.ram_total_kib.is_none() {
        return vec!["no data".to_string()];
    }

    vec![
        size_pair_text(host.ram_used_kib, host.ram_total_kib),
        format!("{}%", host.ram_percent),
    ]
}

fn format_uptime(seconds: Option<u64>) -> String {
    let Some(seconds) = seconds else {
        return "up -".to_string();
    };
    let total_minutes = seconds / 60;
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;
    if days > 0 {
        format!("up {days}d {hours}:{minutes:02}")
    } else {
        format!("up {hours}:{minutes:02}")
    }
}

fn short_kernel(kernel: Option<&str>) -> String {
    kernel
        .map(|value| {
            value
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn kib_pair(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(used), Some(total)) => format!("{} / {}", kib_to_gib(used), kib_to_gib(total)),
        _ => "-".to_string(),
    }
}

fn kib_to_gib(kib: u64) -> String {
    format!("{:.1}GiB", kib as f64 / 1_048_576.0)
}

fn size_pair_text(used: Option<u64>, total: Option<u64>) -> String {
    format!("{}/{}", compact_size_text(used), compact_size_text(total))
}

fn compact_size_text(kib: Option<u64>) -> String {
    kib.map(kib_to_compact).unwrap_or_else(|| "-".to_string())
}

fn disk_section_line(host: &HostSnapshot, inner_width: u16) -> Line<'static> {
    section_line(
        "disk",
        &[format!("{}%", host.storage_percent)],
        theme::STORAGE,
        inner_width,
    )
}

fn disk_no_data_line(inner_width: u16) -> Line<'static> {
    section_line(
        "disk",
        &["no data".to_string()],
        theme::STORAGE,
        inner_width,
    )
}

fn net_line(host: &HostSnapshot, inner_width: u16) -> Line<'static> {
    section_line(
        "net",
        &[
            format!("↓{}", format_rate(host.net_rx_bytes_per_sec)),
            format!("↑{}", format_rate(host.net_tx_bytes_per_sec)),
        ],
        theme::NETWORK,
        inner_width,
    )
}

fn disk_lines(
    disks: &[DiskSnapshot],
    max_lines: usize,
    inner_width: u16,
    mount_width: usize,
) -> Vec<Line<'static>> {
    let detail_widths = disk_detail_widths(disks, max_lines);
    disks
        .iter()
        .take(max_lines)
        .map(|disk| disk_mount_line(disk, inner_width, mount_width, detail_widths))
        .collect()
}

fn disk_mount_line(
    disk: &DiskSnapshot,
    inner_width: u16,
    mount_width: usize,
    detail_widths: DiskDetailWidths,
) -> Line<'static> {
    let mount = fit_text(&short_mount(&disk.mount), mount_width);
    let detail = disk_detail_text(
        disk.percent,
        Some(disk.used_kib),
        Some(disk.total_kib),
        detail_widths,
    );
    let bar_width = disk_bar_width(inner_width, mount_width, detail.chars().count());

    Line::from(vec![
        Span::raw(" "),
        Span::styled(mount, Style::default().fg(theme::STORAGE)),
        Span::raw(" "),
        Span::styled("│", Style::default().fg(theme::BORDER)),
        Span::styled(
            progress_bar(disk.percent, bar_width),
            Style::default().fg(theme::STORAGE),
        ),
        Span::styled("│", Style::default().fg(theme::BORDER)),
        Span::raw(detail),
    ])
}

#[derive(Debug, Clone, Copy)]
struct DiskDetailWidths {
    percent: usize,
    used: usize,
    total: usize,
}

fn disk_detail_widths(disks: &[DiskSnapshot], max_lines: usize) -> DiskDetailWidths {
    let mut widths = DiskDetailWidths {
        percent: 2,
        used: 1,
        total: 1,
    };

    for disk in disks.iter().take(max_lines) {
        widths.percent = widths.percent.max(disk.percent.to_string().chars().count());
        widths.used = widths
            .used
            .max(compact_size_text(Some(disk.used_kib)).chars().count());
        widths.total = widths
            .total
            .max(compact_size_text(Some(disk.total_kib)).chars().count());
    }

    widths
}

fn disk_detail_text(
    percent: u16,
    used: Option<u64>,
    total: Option<u64>,
    widths: DiskDetailWidths,
) -> String {
    let used = compact_size_text(used);
    let total = compact_size_text(total);
    format!(
        " {:>percent_width$}% {used:>used_width$}/{total:>total_width$} ",
        percent,
        percent_width = widths.percent,
        used_width = widths.used,
        total_width = widths.total
    )
}

fn disk_bar_width(inner_width: u16, mount_width: usize, detail_width: usize) -> usize {
    usize::from(inner_width)
        .saturating_sub(1 + mount_width + 1 + 1 + 1 + detail_width + 2)
        .max(12)
}

fn fit_text(value: &str, width: usize) -> String {
    let char_count = value.chars().count();
    if char_count > width {
        let take = width.saturating_sub(1);
        format!("{}…", value.chars().take(take).collect::<String>())
    } else {
        format!("{value:<width$}")
    }
}

fn short_mount(mount: &str) -> String {
    if mount == "/" {
        "/".to_string()
    } else {
        mount
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(mount)
            .chars()
            .take(12)
            .collect()
    }
}

fn kib_to_gib_short(kib: u64) -> String {
    format!("{:.1}G", kib as f64 / 1_048_576.0)
}

fn kib_to_compact(kib: u64) -> String {
    let gib = kib as f64 / 1_048_576.0;
    if gib >= 1024.0 {
        format!("{:.1}T", gib / 1024.0)
    } else {
        format!("{gib:.1}G")
    }
}

fn format_rate(bytes_per_sec: Option<f64>) -> String {
    let Some(rate) = bytes_per_sec else {
        return "sampling".to_string();
    };
    if rate >= 1_073_741_824.0 {
        format!("{:.1}GiB/s", rate / 1_073_741_824.0)
    } else if rate >= 1_048_576.0 {
        format!("{:.1}MiB/s", rate / 1_048_576.0)
    } else if rate >= 1024.0 {
        format!("{:.1}KiB/s", rate / 1024.0)
    } else {
        format!("{rate:.0}B/s")
    }
}

fn format_refresh_ms(ms: u64) -> String {
    format!("{ms}ms")
}

pub fn snapshot_text(state: &AppState) -> String {
    let mut out = String::new();
    let mode = match state.mode {
        Mode::Mock => "mock",
        Mode::Live => "live",
    };
    out.push_str(&format!(
        "{} | mode={mode} | generated={} | hosts={}\n",
        state.title,
        state
            .generated_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%dT%H:%M:%S%:z"),
        state.hosts.len()
    ));

    for (group, hosts) in grouped_hosts(&state.hosts) {
        out.push_str(&format!("\n[{group}]\n"));
        for host in hosts {
            out.push_str(&format!(
                "- {name} ({role}) status={status} cpu={cpu}% temp={temp} load={load} cores={cores} ram={ram}% {ram_detail} storage={storage}% {storage_detail} disks={disks} net=↓{rx}/↑{tx} host={hostname} kernel={kernel} uptime={uptime} last_seen={seen}\n",
                name = host.name,
                role = host.role,
                status = host.status.badge(),
                cpu = host.cpu_percent,
                temp = temperature_text(host.cpu_temperature_celsius),
                load = load_text(host),
                cores = host.cpu_cores.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                ram = host.ram_percent,
                ram_detail = kib_pair(host.ram_used_kib, host.ram_total_kib),
                storage = host.storage_percent,
                storage_detail = kib_pair(host.storage_used_kib, host.storage_total_kib),
                disks = disk_snapshot_text(&host.disks),
                rx = format_rate(host.net_rx_bytes_per_sec),
                tx = format_rate(host.net_tx_bytes_per_sec),
                hostname = host.hostname.as_deref().unwrap_or("-"),
                kernel = short_kernel(host.kernel.as_deref()),
                uptime = format_uptime(host.uptime_seconds),
                seen = host.last_seen.with_timezone(&chrono::Local).format("%H:%M:%S%:z")
            ));
        }
    }

    out
}

fn disk_snapshot_text(disks: &[DiskSnapshot]) -> String {
    if disks.is_empty() {
        "-".to_string()
    } else {
        disks
            .iter()
            .map(|disk| {
                format!(
                    "{}:{}%:{}/{}",
                    disk.mount,
                    disk.percent,
                    kib_to_gib_short(disk.used_kib),
                    kib_to_gib_short(disk.total_kib)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn grouped_hosts(hosts: &[HostSnapshot]) -> BTreeMap<String, Vec<&HostSnapshot>> {
    let mut groups: BTreeMap<String, Vec<&HostSnapshot>> = BTreeMap::new();
    for host in hosts {
        groups.entry(host.group.clone()).or_default().push(host);
    }
    groups
}

fn panel_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(title.into())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER))
        .style(Style::default().bg(theme::PANEL).fg(theme::TEXT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_header_stays_minimal() {
        let detail = header_detail_text(6, 1_000);
        assert_eq!(
            detail,
            "6 hosts · refresh 1000ms · +/- 100ms · q/ctrl-c/esc exits"
        );
        assert!(!detail.contains("MODE"));
        assert!(!detail.contains("ok"));
        assert!(!detail.contains("attention"));
    }

    #[test]
    fn cpu_detail_shows_temperature_only_when_collected() {
        let with_temp = test_host_with_cpu_temp(Some(88.6));
        assert_eq!(cpu_detail(&with_temp), vec!["88.6°C", "16%"]);

        let without_temp = test_host_with_cpu_temp(None);
        assert_eq!(cpu_detail(&without_temp), vec!["16%"]);
    }

    #[test]
    fn disk_detail_columns_align_to_visible_card_maxima() {
        let disks = vec![
            test_disk("/", 0.1, 225.5, 0),
            test_disk("/mnt/tank", 3.9 * 1024.0, 12.7 * 1024.0, 31),
        ];
        let widths = disk_detail_widths(&disks, disks.len());

        assert_eq!(
            disk_detail_text(
                disks[0].percent,
                Some(disks[0].used_kib),
                Some(disks[0].total_kib),
                widths
            ),
            "  0% 0.1G/225.5G "
        );
        assert_eq!(
            disk_detail_text(
                disks[1].percent,
                Some(disks[1].used_kib),
                Some(disks[1].total_kib),
                widths
            ),
            " 31% 3.9T/ 12.7T "
        );
    }

    #[test]
    fn disk_mount_width_aligns_by_grid_column() {
        let mut hosts = vec![
            test_host_with_disks(vec![test_disk("/", 28.0, 38.0, 73)]),
            test_host_with_disks(vec![test_disk("/", 310.0, 900.0, 34)]),
            test_host_with_disks(vec![test_disk("tank", 3.9 * 1024.0, 12.7 * 1024.0, 31)]),
            test_host_with_disks(vec![test_disk("media", 1.4 * 1024.0, 10.0 * 1024.0, 14)]),
        ];
        hosts[0].name = "Node A".to_string();
        hosts[1].name = "Local".to_string();
        hosts[2].name = "Storage".to_string();
        hosts[3].name = "Node B".to_string();

        assert_eq!(column_disk_mount_widths(&hosts, 2), vec![4, 5]);
    }

    #[test]
    fn available_card_layout_spends_the_full_height_budget() {
        let layout = card_content_layout(22, 1);
        let used_rows = 6
            + layout.cpu_graph_rows
            + layout.ram_graph_rows
            + layout.net_graph_rows
            + layout.disk_rows;
        assert_eq!(used_rows, 22);
        assert_eq!(layout.disk_rows, 1);
        assert_eq!(layout.cpu_graph_rows, layout.ram_graph_rows);
        assert!(layout.net_graph_rows >= 1);
    }

    #[test]
    fn multi_disk_card_preserves_mount_rows_before_graph_height() {
        let layout = card_content_layout(22, 8);
        let used_rows = 6
            + layout.cpu_graph_rows
            + layout.ram_graph_rows
            + layout.net_graph_rows
            + layout.disk_rows;
        assert_eq!(used_rows, 22);
        assert_eq!(layout.disk_rows, 8);
        assert!(layout.cpu_graph_rows >= 2);
        assert!(layout.ram_graph_rows >= 2);
    }

    #[test]
    fn shared_card_layout_keeps_metric_rows_aligned() {
        let layout = card_content_layout(22, 8);
        assert_eq!(layout.disk_rows, 8);
        assert!(layout.cpu_graph_rows.abs_diff(layout.ram_graph_rows) <= 1);
        assert!(layout.ram_graph_rows.abs_diff(layout.net_graph_rows) <= 1);
        assert!(layout.net_graph_rows >= 2);
    }

    #[test]
    fn metric_section_labels_do_not_include_trailing_spaces() {
        let line = section_line("cpu", &["12%".to_string()], theme::CPU, 32);
        assert_eq!(line.spans[1].content.as_ref(), "cpu");
        assert!(!line.spans[1].content.ends_with(' '));

        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.starts_with("─cpu─"), "rendered: {rendered}");
    }

    #[test]
    fn disk_heavy_rows_get_height_before_extra_graph_space() {
        let rows = [
            RowLayoutInfo {
                start: 0,
                end: 2,
                disk_rows: 1,
            },
            RowLayoutInfo {
                start: 2,
                end: 4,
                disk_rows: 1,
            },
            RowLayoutInfo {
                start: 4,
                end: 6,
                disk_rows: 8,
            },
        ];
        let heights = disk_priority_row_heights(48, &rows);
        assert_eq!(heights.iter().sum::<u16>(), 48);
        assert!(heights[2] > heights[0]);
        assert!(heights[2] > heights[1]);
        let nas_inner_height = heights[2].saturating_sub(CARD_BORDER_ROWS);
        assert_eq!(card_content_layout(nas_inner_height, 8).disk_rows, 8);
    }
    #[test]
    fn minimum_terminal_size_matches_btop_style_floor() {
        let requirement = minimum_terminal_size();
        assert_eq!(requirement.width, 80);
        assert_eq!(requirement.height, 24);
    }

    fn test_disk(mount: &str, used_gib: f64, total_gib: f64, percent: u16) -> DiskSnapshot {
        DiskSnapshot {
            mount: mount.to_string(),
            used_kib: (used_gib * 1_048_576.0).round() as u64,
            total_kib: (total_gib * 1_048_576.0).round() as u64,
            percent,
        }
    }

    fn test_host_with_disks(disks: Vec<DiskSnapshot>) -> HostSnapshot {
        let mut host = test_host_with_cpu_temp(None);
        host.disks = disks;
        host
    }

    fn test_host_with_cpu_temp(cpu_temperature_celsius: Option<f32>) -> HostSnapshot {
        HostSnapshot {
            id: "test".to_string(),
            name: "Test".to_string(),
            group: "Test".to_string(),
            role: "test".to_string(),
            status: theme::Status::Healthy,
            cpu_percent: 16,
            ram_percent: 0,
            storage_percent: 0,
            cpu_history: vec![],
            ram_history: vec![],
            storage_history: vec![],
            net_history: vec![],
            net_rx_bytes_per_sec: None,
            net_tx_bytes_per_sec: None,
            net_rx_total_bytes: None,
            net_tx_total_bytes: None,
            last_seen: chrono::Utc::now(),
            hostname: None,
            kernel: None,
            uptime_seconds: None,
            cpu_cores: Some(4),
            cpu_temperature_celsius,
            load_1m: Some(0.1),
            load_5m: Some(0.1),
            load_15m: Some(0.1),
            ram_used_kib: None,
            ram_total_kib: None,
            storage_used_kib: None,
            storage_total_kib: None,
            disks: vec![],
        }
    }
}
