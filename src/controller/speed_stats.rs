// 网速统计控制器（task 005）。
//
// 订阅 clash 流量流（task 004 的 traffic_rx），维护定长环形缓冲，
// 计算纵向缩放峰值，生成 SVG path 命令串（viewbox 坐标系），
// 并格式化实时速率 / 累计流量文本，经事件循环写入 SpeedModel 全局。
//
// 组件与全局定义见 ui/speed-stats.slint；导航区底部最终布局由 task 007 接管。
//
// 注意：本版本 Slint 1.17.1 无内置 Canvas 元素，故改用 Path + viewbox 方案
//       （与 plan.md 中「Canvas 不可用时退路为 Path」一致，且响应式更稳）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::clash::stream::TrafficUpdate;
use crate::event::CoreState;
use crate::MainWindow;
use crate::SpeedModel;

/// 环形缓冲上限（约 60 秒的采样）。
const MAX_POINTS: usize = 60;
/// 图表逻辑尺寸，须与 ui/speed-stats.slint 中的 CHART_W / CHART_H 一致。
const CHART_W: f32 = 176.0;
const CHART_H: f32 = 56.0;

/// 启动网速统计后台任务。必须在 MainWindow 创建之后调用。
pub fn start(window: &MainWindow) {
    // 取得 SpeedModel 全局的弱引用，便于在事件循环闭包中安全取用。
    let speed_model = <SpeedModel<'_> as slint::Global<'_, MainWindow>>::get(window);
    let speed: slint::Weak<SpeedModel<'static>> =
        <SpeedModel<'_> as slint::Global<'_, MainWindow>>::as_weak(&speed_model);

    // 首帧先铺满 60 个零值点，避免组件初始阶段没有曲线路径。
    let initial = zero_samples();
    let (down_area, down_line) = build_paths(&initial, 1.0);
    let (up_area, up_line) = build_paths(&initial, 1.0);
    speed_model.set_down_area_cmd(down_area.into());
    speed_model.set_down_line_cmd(down_line.into());
    speed_model.set_up_area_cmd(up_area.into());
    speed_model.set_up_line_cmd(up_line.into());

    // 核心尚未启动也可能已能订阅（broadcast 发送端常驻），返回 None 时直接退出。
    let mut rx = match crate::clash::stream::traffic_rx() {
        Ok(rx) => rx,
        Err(error) => {
            crate::log::error(format_args!("启动流量统计失败：{error}"));
            return;
        }
    };
    let mut core_state = crate::event::subscribe_core_state();
    let initial_core_state = *core_state.borrow_and_update();
    let shared_core_state: Arc<Mutex<CoreState>> = Arc::new(Mutex::new(initial_core_state));

    crate::runtime::spawn_task(async move {
        let mut current_core_state = initial_core_state;
        let mut up = initial;
        let mut down = zero_samples();

        loop {
            tokio::select! {
                biased;
                result = core_state.changed() => {
                    if result.is_err() {
                        return;
                    }
                    let next = *core_state.borrow_and_update();
                    if next == current_core_state {
                        continue;
                    }
                    current_core_state = next;
                    *shared_core_state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
                    up = zero_samples();
                    down = zero_samples();
                    reset_speed_ui(&speed);
                }
                result = rx.recv() => {
                    let traffic = match result {
                        Ok(traffic) => traffic,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            crate::log::error(format_args!("网速统计跳过 {skipped} 条过期消息"));
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    };

                    let latest_core_state = *core_state.borrow();
                    if latest_core_state != current_core_state {
                        current_core_state = latest_core_state;
                        *shared_core_state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = latest_core_state;
                        up = zero_samples();
                        down = zero_samples();
                        reset_speed_ui(&speed);
                    }
                    if !accepts_traffic(current_core_state, &traffic) {
                        continue;
                    }

                    // 纵向缩放峰值（两序列合并取最大值，至少为 1 避免除零）。
                    let expected_generation = traffic.core_generation;
                    let traffic = traffic.traffic;
                    push_sample(&mut up, traffic.up as f32);
                    push_sample(&mut down, traffic.down as f32);
                    let peak = up
                        .iter()
                        .chain(down.iter())
                        .cloned()
                        .fold(0.0f32, f32::max)
                        .max(1.0);

                    // 生成上传 / 下载的面积与描边路径命令串。
                    let (down_area, down_line) = build_paths(&down, peak);
                    let (up_area, up_line) = build_paths(&up, peak);

                    let up_rate = format_rate(traffic.up);
                    let down_rate = format_rate(traffic.down);
                    let up_total = format_total(traffic.up_total);
                    let down_total = format_total(traffic.down_total);

                    // 克隆弱引用供事件循环闭包使用（外层 speed 仍需保留给后续循环）。
                    let weak = speed.clone();
                    let shared_core_state = shared_core_state.clone();
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        let state = shared_core_state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if !state.running || state.generation != expected_generation {
                            return;
                        }
                        drop(state);
                        if let Some(speed) = weak.upgrade() {
                            speed.set_down_area_cmd(down_area.into());
                            speed.set_down_line_cmd(down_line.into());
                            speed.set_up_area_cmd(up_area.into());
                            speed.set_up_line_cmd(up_line.into());
                            speed.set_up_rate(up_rate.into());
                            speed.set_down_rate(down_rate.into());
                            speed.set_up_total(up_total.into());
                            speed.set_down_total(down_total.into());
                        }
                    }) {
                        crate::log::error(format_args!("网速统计 UI 回调失败：{error}"));
                        return;
                    }
                }
            }
        }
    });
}

fn reset_speed_ui(speed: &slint::Weak<SpeedModel<'static>>) {
    let initial = zero_samples();
    let (down_area, down_line) = build_paths(&initial, 1.0);
    let (up_area, up_line) = build_paths(&initial, 1.0);
    let weak = speed.clone();
    if let Err(error) = slint::invoke_from_event_loop(move || {
        if let Some(speed) = weak.upgrade() {
            speed.set_down_area_cmd(down_area.into());
            speed.set_down_line_cmd(down_line.into());
            speed.set_up_area_cmd(up_area.into());
            speed.set_up_line_cmd(up_line.into());
            speed.set_up_rate("0 B/s".into());
            speed.set_down_rate("0 B/s".into());
            speed.set_up_total("0 B".into());
            speed.set_down_total("0 B".into());
        }
    }) {
        crate::log::error(format_args!("重置网速统计 UI 失败：{error}"));
    }
}

fn accepts_traffic(state: CoreState, update: &TrafficUpdate) -> bool {
    state.running && state.generation == update.core_generation
}

fn zero_samples() -> VecDeque<f32> {
    VecDeque::from(vec![0.0; MAX_POINTS])
}

fn push_sample(samples: &mut VecDeque<f32>, value: f32) {
    samples.push_back(value);
    samples.pop_front();
}

/// 根据采样序列生成 (面积 path, 描边 path) 两个 SVG 命令串。
/// 坐标位于 viewbox 空间 0..CHART_W × 0..CHART_H。
fn build_paths(vals: &VecDeque<f32>, peak: f32) -> (String, String) {
    let Some((first, segments)) = curve_segments(vals, peak) else {
        return (String::new(), String::new());
    };

    // 面积：自底部左下角起，沿平滑曲线到右下角，闭合。
    let mut area = format!(
        "M 0.00 {CHART_H:.2} L {x:.2} {y:.2} ",
        x = first.x,
        y = first.y
    );
    // 描边：从第一个采样点开始使用 cubic Bézier 分段。
    let mut line = format!("M {x:.2} {y:.2} ", x = first.x, y = first.y);

    for segment in segments {
        let command = format!(
            "C {c1x:.2} {c1y:.2} {c2x:.2} {c2y:.2} {x:.2} {y:.2} ",
            c1x = segment.control_1.x,
            c1y = segment.control_1.y,
            c2x = segment.control_2.x,
            c2y = segment.control_2.y,
            x = segment.end.x,
            y = segment.end.y,
        );
        line.push_str(&command);
        area.push_str(&command);
    }
    area.push_str(&format!("L {CHART_W:.2} {CHART_H:.2} Z"));

    (area, line)
}

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

struct CurveSegment {
    control_1: Point,
    control_2: Point,
    end: Point,
}

fn curve_segments(vals: &VecDeque<f32>, peak: f32) -> Option<(Point, Vec<CurveSegment>)> {
    let n = vals.len();
    if n == 0 {
        return None;
    }

    let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
    let values = vals
        .iter()
        .map(|value| {
            if value.is_finite() {
                value.max(0.0).min(peak)
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let scale = (CHART_H - 4.0) / peak;
    let y_values = values
        .iter()
        .map(|value| (CHART_H - 2.0 - value * scale).clamp(0.0, CHART_H))
        .collect::<Vec<_>>();
    let first = Point {
        x: 0.0,
        y: y_values[0],
    };
    if n == 1 {
        return Some((first, Vec::new()));
    }

    let tangents = monotone_tangents(&y_values);
    let denominator = (n - 1) as f32;
    let step = CHART_W / denominator;
    let mut segments = Vec::with_capacity(n - 1);
    for index in 0..(n - 1) {
        let start = Point {
            x: index as f32 * step,
            y: y_values[index],
        };
        let end = Point {
            x: (index + 1) as f32 * step,
            y: y_values[index + 1],
        };
        let control_1 = Point {
            x: start.x + step / 3.0,
            y: clamp_between(start.y + tangents[index] / 3.0, start.y, end.y),
        };
        let control_2 = Point {
            x: end.x - step / 3.0,
            y: clamp_between(end.y - tangents[index + 1] / 3.0, start.y, end.y),
        };
        segments.push(CurveSegment {
            control_1,
            control_2,
            end,
        });
    }
    Some((first, segments))
}

/// 为等距采样计算单调 Hermite 切线，避免局部极值附近的曲线过冲。
fn monotone_tangents(values: &[f32]) -> Vec<f32> {
    if values.len() < 2 {
        return vec![0.0; values.len()];
    }
    let secants = values
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let mut tangents = vec![0.0; values.len()];
    tangents[0] = secants[0];
    tangents[values.len() - 1] = secants[secants.len() - 1];
    for index in 1..(values.len() - 1) {
        let left = secants[index - 1];
        let right = secants[index];
        if left == 0.0 || right == 0.0 || left.signum() != right.signum() {
            continue;
        }
        let tangent = 2.0 * left * right / (left + right);
        tangents[index] = if tangent.is_finite() { tangent } else { 0.0 };
    }
    tangents
}

fn clamp_between(value: f32, first: f32, second: f32) -> f32 {
    value.clamp(first.min(second), first.max(second))
}

/// 速率自适应单位：B/s · KB/s · MB/s
fn format_rate(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B/s")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB/s", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB/s", bytes as f64 / 1024.0 / 1024.0)
    }
}

/// 累计流量自适应单位：B · KB · MB · GB
fn format_total(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[f32]) -> VecDeque<f32> {
        values.iter().copied().collect()
    }

    fn assert_finite_path(path: &str) {
        for value in path
            .split_whitespace()
            .filter_map(|token| token.parse::<f32>().ok())
        {
            assert!(value.is_finite());
            assert!((0.0..=CHART_W.max(CHART_H)).contains(&value));
        }
    }

    fn assert_safe_curve(values: &[f32]) {
        let values = samples(values);
        let (area, line) = build_paths(&values, 10.0);
        assert_eq!(line.matches('C').count(), values.len().saturating_sub(1));
        assert!(area.ends_with(" Z"));
        assert_finite_path(&area);
        assert_finite_path(&line);
        let (_, segments) = curve_segments(&values, 10.0).unwrap();
        for segment in segments {
            let low = segment.end.y.min(segment.control_1.y);
            let high = segment.end.y.max(segment.control_1.y);
            assert!((low..=high).contains(&segment.control_2.y));
        }
    }

    #[test]
    fn initial_samples_have_fixed_zero_length() {
        let samples = zero_samples();
        assert_eq!(samples.len(), MAX_POINTS);
        assert!(samples.iter().all(|value| *value == 0.0));
        let (_, line) = build_paths(&samples, 1.0);
        assert_eq!(line.matches('C').count(), MAX_POINTS - 1);
    }

    #[test]
    fn sample_update_keeps_fixed_length() {
        let mut samples = zero_samples();
        push_sample(&mut samples, 42.0);
        assert_eq!(samples.len(), MAX_POINTS);
        assert_eq!(samples.back(), Some(&42.0));
        assert_eq!(samples.front(), Some(&0.0));
    }

    #[test]
    fn cubic_curve_handles_zero_step_peak_and_platform() {
        assert_safe_curve(&vec![0.0; MAX_POINTS]);

        let mut step = vec![0.0; MAX_POINTS];
        step[MAX_POINTS / 2] = 10.0;
        assert_safe_curve(&step);

        let mut peak = vec![2.0; MAX_POINTS];
        peak[MAX_POINTS / 2] = 10.0;
        assert_safe_curve(&peak);

        assert_safe_curve(&[0.0, 5.0, 5.0, 2.0, 2.0]);
    }

    #[test]
    fn non_finite_samples_do_not_reach_path() {
        assert_safe_curve(&[f32::NAN, f32::INFINITY, -f32::INFINITY, -1.0]);
        assert_safe_curve(&[f32::MAX, 0.0, f32::MIN_POSITIVE]);
    }

    #[test]
    fn traffic_from_old_core_generation_is_rejected() {
        let update = TrafficUpdate {
            core_generation: 1,
            traffic: crate::clash::api::Traffic {
                up: 1,
                down: 2,
                up_total: 3,
                down_total: 4,
            },
        };
        assert!(!accepts_traffic(
            CoreState {
                running: true,
                generation: 2,
            },
            &update
        ));
        assert!(!accepts_traffic(
            CoreState {
                running: false,
                generation: 1,
            },
            &update
        ));
        assert!(accepts_traffic(
            CoreState {
                running: true,
                generation: 1,
            },
            &update
        ));
    }
}
