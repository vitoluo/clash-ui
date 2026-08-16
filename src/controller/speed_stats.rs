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

use slint::ComponentHandle;
use slint::Global;

use crate::MainWindow;
use crate::SpeedModel;

/// 环形缓冲上限（约 60 秒的采样）。
const MAX_POINTS: usize = 60;
/// 图表逻辑尺寸，须与 ui/speed-stats.slint 中的 CHART_W / CHART_H 一致。
const CHART_W: f32 = 176.0;
const CHART_H: f32 = 56.0;

/// 启动网速统计后台线程。必须在 MainWindow 创建之后调用。
pub fn start(window: &MainWindow) {
    // 取得 SpeedModel 全局的弱引用，便于在事件循环闭包中安全取用。
    let speed = window.global::<SpeedModel>().as_weak();

    // 核心尚未启动也可能已能订阅（broadcast 发送端常驻），返回 None 时直接退出。
    let Some(mut rx) = crate::clash::api::traffic_rx() else {
        return;
    };

    std::thread::spawn(move || {
        let mut up: Vec<f32> = Vec::with_capacity(MAX_POINTS);
        let mut down: Vec<f32> = Vec::with_capacity(MAX_POINTS);

        loop {
            // 在全局 tokio runtime 上阻塞接收下一帧流量。
            let traffic = match crate::clash::api::block(async { rx.recv().await }) {
                Ok(t) => t,
                // lagged/closed 等异常：继续下一个循环。
                Err(_) => continue,
            };

            up.push(traffic.up as f32);
            down.push(traffic.down as f32);
            if up.len() > MAX_POINTS {
                up.remove(0);
                down.remove(0);
            }

            // 纵向缩放峰值（两序列合并取最大值，至少为 1 避免除零）。
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
            slint::invoke_from_event_loop(move || {
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
            })
            .expect("事件循环已关闭，无法更新 SpeedModel");
        }
    });
}

/// 根据采样序列生成 (面积 path, 描边 path) 两个 SVG 命令串。
/// 坐标位于 viewbox 空间 0..CHART_W × 0..CHART_H。
fn build_paths(vals: &[f32], peak: f32) -> (String, String) {
    let n = vals.len();
    if n == 0 {
        return (String::new(), String::new());
    }
    let denom = if n > 1 { (n - 1) as f32 } else { 1.0 };
    let w = CHART_W;
    let h = CHART_H;
    let peak = peak.max(1.0);

    // 面积：自底部左下角起，沿曲线到右下角，闭合。
    let mut area = format!("M 0.00 {h:.2} ");
    // 描边：自第一个点起的折线。
    let mut line = String::new();

    for (i, &v) in vals.iter().enumerate() {
        let x = (i as f32) / denom * w;
        let y = (h - 2.0) - (v / peak) * (h - 4.0);
        if i == 0 {
            line.push_str(&format!("M {x:.2} {y:.2} "));
        } else {
            line.push_str(&format!("L {x:.2} {y:.2} "));
        }
        area.push_str(&format!("L {x:.2} {y:.2} "));
    }
    area.push_str(&format!("L {w:.2} {h:.2} Z"));

    (area, line)
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
