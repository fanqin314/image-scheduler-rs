//! 车组 V2V 可视化面板后端。
//!
//! 用一个 [`Fleet`] 在固定 UDP 端口监听车队广播（复用 `v2v` 通信层），
//! 浏览器通过 `/ws/v2v` 的 WebSocket 每 500ms 收到一次车队快照。
//! 通过 WebSocket 断开前的最后消息时间判断在线/离线（>500ms 判离线）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use v2v::types::{MessagePayload, VehicleState};
use v2v::V2vComm;

use crate::AppState;

/// 面板监听端口：车队车辆也会把广播打到该端口（见 `simulator --hub-port`）。
pub const HUB_PORT: u16 = 9100;
/// 离线判定阈值（毫秒）。
const STALE_MS: Duration = Duration::from_millis(500);

/// 车队聚合节点。
pub struct Fleet {
    comm: Arc<V2vComm>,
    latest_state: Arc<Mutex<HashMap<String, VehicleState>>>,
}

impl Fleet {
    /// 绑定 UDP 端口并后台接收。绑定失败（端口占用）返回 `None`，面板显示为空。
    pub async fn spawn(hub_port: u16) -> Option<Arc<Self>> {
        let comm = match V2vComm::bind("v2v-hub", hub_port, vec![]).await {
            Ok(c) => c,
            Err(_) => return None,
        };
        let latest_state = Arc::new(Mutex::new(HashMap::new()));
        let ls = Arc::clone(&latest_state);
        // 把收到的 State 存一份供面板展示
        comm.set_handler(move |m| {
            if let MessagePayload::State(s) = m.payload.clone() {
                ls.lock().unwrap().insert(m.sender.clone(), s);
            }
        });
        Some(Arc::new(Fleet { comm, latest_state }))
    }

    /// 生成车队快照（按 id 排序）。
    fn snapshot(&self) -> Vec<serde_json::Value> {
        let now = Instant::now();
        let seen = self.comm.last_seen();
        let states = self.latest_state.lock().unwrap();
        let mut cars: Vec<serde_json::Value> = seen
            .iter()
            .map(|(id, t)| {
                let age_ms = now.duration_since(*t).as_millis() as u64;
                let online = age_ms <= STALE_MS.as_millis() as u64;
                let st = states.get(id);
                serde_json::json!({
                    "id": id,
                    "online": online,
                    "age_ms": age_ms,
                    "x": st.map(|s| s.x).unwrap_or(0.0),
                    "y": st.map(|s| s.y).unwrap_or(0.0),
                    "speed": st.map(|s| s.speed).unwrap_or(0.0),
                    "heading": st.map(|s| s.heading).unwrap_or(0.0),
                    "load": st.map(|s| s.load).unwrap_or(0.0),
                    "received": self.comm.incoming_count(id),
                })
            })
            .collect();
        cars.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        cars
    }
}

/// `/ws/v2v`：升级 WebSocket，每 500ms 推送一次快照。
pub async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // 该路由仅在 fleet 可用时注册，故此处必有值。
    let fleet = state
        .fleet
        .expect("fleet 恒有值：/ws/v2v 仅在 fleet 可用时注册");
    ws.on_upgrade(move |socket| panel_stream(socket, fleet))
}

async fn panel_stream(mut socket: WebSocket, fleet: Arc<Fleet>) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.tick().await; // 立即推一帧
    loop {
        interval.tick().await;
        let snapshot = serde_json::json!({ "cars": fleet.snapshot() }).to_string();
        if socket.send(Message::Text(snapshot.into())).await.is_err() {
            break; // 客户端断开
        }
    }
}

/// `/v2v`：返回可视化面板 HTML。
pub async fn panel_page() -> impl IntoResponse {
    axum::response::Html(PANEL_HTML)
}

const PANEL_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>车组 V2V 通信面板</title>
<style>
  :root { --bg:#0f1520; --card:#1a2332; --line:#2a3650; --txt:#dbe4f0; --mut:#8aa0c0;
          --ok:#3ddc8c; --warn:#ffb454; --off:#5b6b85; --acc:#4a8df0; }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--txt);
         font-family:"Segoe UI", system-ui, -apple-system, sans-serif; padding:24px; }
  header { display:flex; align-items:center; gap:12px; margin-bottom:18px; }
  header h1 { font-size:18px; margin:0; font-weight:600; letter-spacing:.5px; }
  .dot { width:10px; height:10px; border-radius:50%; background:var(--off); }
  .dot.live { background:var(--ok); box-shadow:0 0 8px var(--ok); animation:pulse 1.6s infinite; }
  @keyframes pulse { 0%,100%{opacity:1;} 50%{opacity:.4;} }
  .sub { color:var(--mut); font-size:13px; }
  #grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(240px,1fr)); gap:14px; }
  .card { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:16px;
          transition:transform .18s ease, border-color .18s ease; }
  .card:hover { transform:translateY(-2px); }
  .card.off { opacity:.55; border-color:transparent; }
  .row { display:flex; justify-content:space-between; padding:5px 0; font-size:14px; color:var(--txt); }
  .row .k { color:var(--mut); }
  .card h2 { display:flex; align-items:center; gap:8px; font-size:15px; margin:0 0 10px; }
  .pill { font-size:12px; padding:2px 8px; border-radius:20px; background:var(--line); color:var(--mut); }
  .pill.ok { background:rgba(61,220,140,.15); color:var(--ok); }
  .pill.warn { background:rgba(255,180,84,.15); color:var(--warn); }
  .loadbar { height:6px; border-radius:3px; background:var(--line); overflow:hidden; margin-top:4px; }
  .loadbar > i { display:block; height:100%; background:var(--acc); transition:width .4s linear; }
  .empty { color:var(--mut); text-align:center; padding:40px 0; }
  .hint { margin-top:14px; color:var(--mut); font-size:12px; line-height:1.7; }
  code { background:var(--line); padding:1px 6px; border-radius:4px; font-size:12px; }
</style>
</head>
<body>
  <header>
    <span class="dot live" id="conn"></span>
    <h1>车组 V2V 通信面板</h1>
    <span class="sub" id="sub">连接中…</span>
  </header>
  <div id="grid"><div class="empty">等待车队广播… 运行 <code>simulator --hub-port 9100</code></div></div>
  <div class="hint">
    面板在 UDP <b>9100</b> 端口直接监听车队广播（复用 v2v 通信层）。
    车辆超过 500ms 无消息即判为离线。<br>
    启动演示：<code>image-scheduler-rs</code> 后打开本页，再运行
    <code>vehicle --id car-a --listen-port 9000 --peers 127.0.0.1:9001,127.0.0.1:9100 --known car-b</code> 等。
  </div>
<script>
(function(){
  var grid=document.getElementById('grid'), conn=document.getElementById('conn'),
      sub=document.getElementById('sub');
  function ws(){ var s=new WebSocket((location.protocol==='https:'?'wss://':'ws://')+location.host+'/ws/v2v');
    s.onopen=function(){ conn.classList.add('live'); sub.textContent='已连接，实时刷新'; };
    s.onclose=function(){ conn.classList.remove('live'); sub.textContent='已断开，重连中…';
      setTimeout(ws, 1000); };
    s.onmessage=function(e){ render(JSON.parse(e.data)); };
  }
  function esc(x){ return String(x).replace(/[&<>"]/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c];}); }
  function render(d){
    var cars=d.cars||[];
    if(!cars.length){ grid.innerHTML='<div class="empty">等待车队广播…</div>'; return; }
    grid.innerHTML=cars.map(function(c){
      var off=!c.online;
      var load=Math.max(0,Math.min(100,c.load*100)).toFixed(0);
      var cls=off?'off':'';
      var pill= c.online?'<span class="pill ok">在线</span>':'<span class="pill warn">离线 '+c.age_ms+'ms</span>';
      return '<div class="card '+cls+'"><h2><span class="dot '+(c.online?'':'')+'"></span>'+esc(c.id)+' '+pill+'</h2>'
        +'<div class="row"><span class="k">速度</span><span>'+c.speed.toFixed(1)+' m/s</span></div>'
        +'<div class="row"><span class="k">航向</span><span>'+c.heading.toFixed(0)+'°</span></div>'
        +'<div class="row"><span class="k">位置</span><span>('+c.x.toFixed(1)+', '+c.y.toFixed(1)+')</span></div>'
        +'<div class="row"><span class="k">已收</span><span>'+c.received+' 条</span></div>'
        +'<div class="row"><span class="k">算力负载</span><span>'+load+'%</span></div>'
        +'<div class="loadbar" aria-hidden="true"><i style="width:'+load+'%"></i></div>'
        +'</div>';
    }).join('');
  }
  ws();
})();
</script>
</body>
</html>"#;