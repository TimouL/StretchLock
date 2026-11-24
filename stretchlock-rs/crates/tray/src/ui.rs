//! The settings page embedded as an HTML string and loaded into the wry
//! WebView (WebView2). The page talks to the host over the wry IPC shim:
//! uplink `window.ipc.postMessage(JSON)` -> `settings::UiEvent`, downlink
//! `window.__stretchlock.receive(obj)` pushed via `evaluate_script`.

/// Win11-styled settings page. Tabs: General / Security / Advanced; the
/// stop-service passphrase prompt is an in-page modal.
pub(crate) const SETTINGS_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>StretchLock 设置</title>
<style>
  :root {
    color-scheme: light dark;
    --bg: #f3f3f3; --card: #ffffff; --layer: #fbfbfb;
    --text: #1a1a1a; --sub: #5f5f5f;
    --border: #e6e6e6; --row: #ebebeb;
    --accent: #0067c0; --accent-hover: #1975c6;
    --danger: #c42b1c; --danger-hover: #d13438;
    --field: #ffffff; --field-border: #d6d6d6;
    --hover: rgba(0,0,0,.05);
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --bg: #202020; --card: #2b2b2b; --layer: #272727;
      --text: #ffffff; --sub: #a6a6a6;
      --border: #3d3d3d; --row: #383838;
      --accent: #4cc2ff; --accent-hover: #6ac9ff;
      --danger: #ff99a4; --danger-hover: #ff99a4;
      --field: #2d2d2d; --field-border: #454545;
      --hover: rgba(255,255,255,.06);
    }
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  html, body { height: 100%; }
  body {
    font-family: 'Segoe UI Variable Display','Segoe UI Variable','Segoe UI',system-ui,sans-serif;
    font-size: 13px; color: var(--text); background: var(--bg);
    overflow: hidden; user-select: none; cursor: default;
    display: flex; flex-direction: column;
  }
  /* ---- title bar ---- */
  #titlebar {
    height: 38px; flex: none; display: flex; align-items: center;
    padding-left: 14px; background: var(--bg);
  }
  #titlebar .title { font-size: 13px; font-weight: 600; flex: 1; }
  #titlebar .caption {
    width: 12px; height: 12px; border-radius: 3px; margin-right: 8px;
    background: linear-gradient(135deg, #2563eb 55%, #1d4fd7);
    position: relative; flex: none;
  }
  #titlebar .caption::after {
    content: ''; position: absolute; inset: 2px; border-radius: 1px;
    border-left: 3px solid #f5f7fa; border-bottom: 3px solid #f5f7fa;
    transform: rotate(-12deg);
  }
  #closeBtn {
    width: 44px; height: 38px; border: none; background: transparent;
    color: var(--text); font-size: 12px; cursor: pointer; flex: none;
    font-family: 'Segoe Fluent Icons','Segoe MDL2 Assets',sans-serif;
  }
  #closeBtn:hover { background: #c42b1c; color: #fff; }
  /* ---- body ---- */
  #page { flex: 1; overflow-y: auto; padding: 4px 16px 0; }
  .card {
    background: var(--card); border: 1px solid var(--border);
    border-radius: 8px; padding: 4px 14px; margin-bottom: 12px;
  }
  .row {
    display: flex; align-items: center; justify-content: space-between;
    gap: 12px; min-height: 44px; padding: 4px 0;
  }
  .row + .row { border-top: 1px solid var(--row); }
  .row .label { flex: 1; }
  .row .label .sub { color: var(--sub); font-size: 12px; margin-top: 1px; }
  /* status area */
  #statusCard { display: flex; align-items: center; gap: 12px; min-height: 58px; padding: 8px 14px; }
  #clock { font-family: 'Segoe UI Variable Display','Segoe UI',system-ui,sans-serif;
           font-size: 26px; font-weight: 600; letter-spacing: .5px; font-variant-numeric: tabular-nums; }
  #statusText { color: var(--sub); font-size: 12px; }
  #statusSide { margin-left: auto; text-align: right; color: var(--sub); font-size: 12px; line-height: 1.5; }
  /* tabs */
  #tabs { display: flex; gap: 4px; margin-bottom: 12px; }
  #tabs button {
    border: 1px solid transparent; background: transparent; color: var(--text);
    font: inherit; padding: 5px 12px; border-radius: 5px; cursor: pointer;
  }
  #tabs button:hover { background: var(--hover); }
  #tabs button[aria-selected='true'] { background: var(--card); border-color: var(--border); font-weight: 600; }
  /* controls */
  input[type='number'], input[type='password'], select {
    font: inherit; color: var(--text); background: var(--field);
    border: 1px solid var(--field-border); border-bottom-color: var(--sub);
    border-radius: 4px; height: 30px; padding: 0 8px; outline: none;
  }
  input[type='number'] { width: 58px; text-align: center; }
  input:focus, select:focus { border-bottom: 2px solid var(--accent); }
  input::-webkit-outer-spin-button, input::-webkit-inner-spin-button { -webkit-appearance: none; }
  .stepper { display: inline-flex; align-items: center; gap: 4px; }
  .stepper button {
    width: 28px; height: 30px; border: 1px solid var(--field-border);
    border-radius: 4px; background: var(--field); color: var(--text);
    font: inherit; cursor: pointer; font-size: 14px; line-height: 1;
  }
  .stepper button:hover { background: var(--hover); }
  /* toggle switch */
  .switch { position: relative; width: 40px; height: 20px; flex: none; }
  .switch input { display: none; }
  .switch .track {
    position: absolute; inset: 0; border-radius: 10px;
    border: 1px solid var(--sub); background: transparent; transition: background .1s;
  }
  .switch .track::after {
    content: ''; position: absolute; top: 3px; left: 3px; width: 12px; height: 12px;
    border-radius: 50%; background: var(--sub); transition: all .1s;
  }
  .switch input:checked + .track { background: var(--accent); border-color: var(--accent); }
  .switch input:checked + .track::after { left: 23px; background: #fff; }
  /* buttons */
  .btn {
    font: inherit; height: 30px; padding: 0 16px; border-radius: 4px; cursor: pointer;
    border: 1px solid var(--field-border); background: var(--card); color: var(--text);
  }
  .btn:hover { background: var(--hover); }
  .btn.primary { background: var(--accent); border-color: var(--accent); color: #fff; font-weight: 600; }
  .btn.primary:hover { background: var(--accent-hover); }
  .btn.danger { background: var(--danger); border-color: var(--danger); color: #fff; font-weight: 600; }
  .btn.danger:hover { background: var(--danger-hover); }
  /* security tab */
  #messages {
    width: 100%; min-height: 132px; resize: vertical; box-sizing: border-box;
    font-family: inherit; font-size: 13px; line-height: 1.5; padding: 8px 10px;
    border-radius: 6px; border: 1px solid var(--field-border);
    background: var(--field); color: var(--text);
  }
  #messages:focus { outline: none; border-color: var(--accent); }
  .pfield { margin: 6px 0 12px; }
  .pfield label { display: block; margin-bottom: 4px; color: var(--sub); font-size: 12px; }
  .pfield input { width: 100%; }
  #pwError, #saveError { color: var(--danger); font-size: 12px; margin: 4px 0 0; min-height: 15px; }
  .note { color: var(--sub); font-size: 12px; line-height: 1.5; margin: 2px 0 12px; }
  #changePw { margin-bottom: 4px; }
  /* footer */
  #footer {
    flex: none; display: flex; justify-content: flex-end; gap: 8px;
    padding: 10px 16px 14px; background: var(--bg);
  }
  /* modal */
  #stopModal {
    position: fixed; inset: 0; display: none; align-items: center; justify-content: center;
    background: rgba(0,0,0,.4); z-index: 10;
  }
  #stopModal.open { display: flex; }
  #stopModal .sheet {
    width: 340px; background: var(--card); border: 1px solid var(--border);
    border-radius: 8px; padding: 18px 18px 14px; box-shadow: 0 8px 24px rgba(0,0,0,.25);
  }
  #stopModal h2 { font-size: 15px; margin-bottom: 8px; }
  #stopModal p { color: var(--sub); font-size: 12px; line-height: 1.5; margin-bottom: 10px; }
  #stopModal input { width: 100%; margin-bottom: 4px; }
  #stopError { color: var(--danger); font-size: 12px; margin: 2px 0 8px; min-height: 15px; }
  #stopModal .actions { display: flex; justify-content: flex-end; gap: 8px; }
  /* toast */
  #toast {
    position: fixed; bottom: 56px; left: 50%; transform: translateX(-50%) translateY(8px);
    background: var(--card); border: 1px solid var(--border); border-radius: 5px;
    padding: 7px 16px; font-size: 12px; opacity: 0; pointer-events: none;
    transition: opacity .15s, transform .15s; box-shadow: 0 4px 12px rgba(0,0,0,.15);
  }
  #toast.show { opacity: 1; transform: translateX(-50%) translateY(0); }
  .hidden { display: none !important; }
</style>
</head>
<body>
  <div id='titlebar'>
    <div class='caption'></div>
    <div class='title'>StretchLock</div>
    <button id='closeBtn' title='关闭'>&#xE8BB;</button>
  </div>

  <div id='page'>
    <div class='card' id='statusCard'>
      <div>
        <div id='clock'>--:--</div>
        <div id='statusText'>正在连接服务…</div>
      </div>
      <div id='statusSide'>
        今日休息 <b id='statBreaks'>-</b> 次<br>
        连续打卡 <b id='statStreak'>-</b> 天
      </div>
    </div>

    <div id='tabs' role='tablist'>
      <button role='tab' data-tab='general' aria-selected='true'>常规</button>
      <button role='tab' data-tab='security' aria-selected='false'>安全</button>
      <button role='tab' data-tab='appearance' aria-selected='false'>外观</button>
      <button role='tab' data-tab='advanced' aria-selected='false'>高级</button>
    </div>

    <div id='tab-general'>
      <div class='card'>
        <div class='row'>
          <div class='label'>启用计时器</div>
          <label class='switch'><input type='checkbox' id='enabled'><span class='track'></span></label>
        </div>
        <div class='row'>
          <div class='label'>工作间隔<span class='sub'>连续工作多久后提醒休息（1-480 分钟）</span></div>
          <div class='stepper'>
            <button data-step='-1' data-for='work'>−</button>
            <input type='number' id='work' min='1' max='480'>
            <button data-step='1' data-for='work'>+</button>
          </div>
        </div>
        <div class='row'>
          <div class='label'>休息时长<span class='sub'>每次休息持续的时间（1-60 分钟）</span></div>
          <div class='stepper'>
            <button data-step='-1' data-for='break'>−</button>
            <input type='number' id='break' min='1' max='60'>
            <button data-step='1' data-for='break'>+</button>
          </div>
        </div>
      </div>
    </div>

    <div id='tab-security' class='hidden'>
      <div class='card'>
        <div class='row' style='border-top:none; min-height:36px;'>
          <div class='label'>修改口令<span class='sub'>停止服务时需要输入口令确认</span></div>
        </div>
        <div class='pfield'><label>当前口令</label><input type='password' id='pwOld' autocomplete='off'></div>
        <div class='pfield'><label>新口令</label><input type='password' id='pwNew' autocomplete='off'></div>
        <div class='pfield'><label>确认新口令</label><input type='password' id='pwConfirm' autocomplete='off'></div>
        <button class='btn' id='changePw'>修改口令</button>
        <div id='pwError'></div>
        <p class='note'>口令以 PBKDF2-HMAC-SHA256（10 万次迭代 + 随机盐）散列存储，无法还原明文；修改将在保存设置后生效。</p>
      </div>
    </div>

    <div id='tab-appearance' class='hidden'>
      <div class='card'>
        <div class='row'>
          <div class='label'>卡通动画<span class='sub'>休息遮罩上会动的小角色</span></div>
          <label class='switch'><input type='checkbox' id='animation'><span class='track'></span></label>
        </div>
        <div class='row'>
          <div class='label'>内置角色<span class='sub'>猫 / 狗 / 恐龙；放入自定义 GIF 优先显示</span></div>
          <select id='mascot'>
            <option value='cat'>猫</option>
            <option value='dog'>狗</option>
            <option value='dino'>恐龙</option>
          </select>
        </div>
        <div class='row' style='display:block; border-top:none;'>
          <div class='label' style='margin-bottom:6px;'>休息文案<span class='sub'>每行一条，每次休息随机顺序轮换；留空则恢复默认</span></div>
          <textarea id='messages' spellcheck='false' placeholder='哎呀，时间到啦！眼睛辛苦这么久，起来走两步吧～'></textarea>
        </div>
        <div class='row' style='display:block; border-top:none;'>
          <div class='label' style='margin-bottom:6px;'>动画素材<span class='sub'>把 GIF 改名 mascot.gif，或把 PNG 序列帧放入 %APPDATA%\\StretchLock\\sprites\\ 目录；支持任意尺寸、自动按 GIF 帧率播放</span></div>
        </div>
      </div>
    </div>

    <div id='tab-advanced' class='hidden'>
      <div class='card'>
        <div class='row'>
          <div class='label'>日志级别</div>
          <select id='logLevel'>
            <option value='DEBUG'>DEBUG</option>
            <option value='INFO'>INFO</option>
            <option value='WARNING'>WARNING</option>
            <option value='ERROR'>ERROR</option>
            <option value='OFF'>OFF</option>
          </select>
        </div>
        <div class='row'>
          <div class='label'>服务开机自启<span class='sub'>守护进程随 Windows 启动</span></div>
          <label class='switch'><input type='checkbox' id='autostartDaemon'><span class='track'></span></label>
        </div>
        <div class='row'>
          <div class='label'>托盘开机自启<span class='sub'>托盘图标随 Windows 启动</span></div>
          <label class='switch'><input type='checkbox' id='autostartTray'><span class='track'></span></label>
        </div>
      </div>
    </div>
  </div>

  <div id='footer'>
    <button class='btn' id='stopBtn'>停止服务…</button>
    <button class='btn primary' id='saveBtn'>保存设置</button>
  </div>

  <div id='stopModal'>
    <div class='sheet'>
      <h2 id='promptTitle'>停止 StretchLock 服务</h2>
      <p id='promptDesc'>停止后休息提醒将暂停，直到重新启动服务。请输入口令确认。</p>
      <input type='password' id='stopPassphrase' placeholder='口令' autocomplete='off'>
      <div id='stopError'></div>
      <div class='actions'>
        <button class='btn' id='stopCancel'>取消</button>
        <button class='btn danger' id='promptConfirm'>确认</button>
      </div>
    </div>
  </div>

  <div id='toast'></div>

<script>
'use strict';

function send(obj) {
  if (window.ipc) { window.ipc.postMessage(JSON.stringify(obj)); }
}

function $(id) { return document.getElementById(id); }

function fmt(sec) {
  sec = Math.max(0, sec | 0);
  var m = (sec / 60) | 0, s = sec % 60;
  return (m < 10 ? '0' + m : m) + ':' + (s < 10 ? '0' + s : s);
}

function updateStatus(st) {
  var clock = $('clock'), text = $('statusText');
  if (!st) {
    clock.textContent = '--:--';
    text.textContent = '无法连接服务';
    return;
  }
  if (!st.enabled) {
    clock.textContent = '--:--';
    text.textContent = '计时器已禁用';
  } else if (st.state === 'breaking') {
    clock.textContent = fmt(st.remaining_seconds);
    text.textContent = '休息中…';
  } else if (st.state === 'awaiting_resume') {
    clock.textContent = '--:--';
    text.textContent = '休息完成';
  } else {
    clock.textContent = fmt(st.remaining_seconds);
    text.textContent = '距下次休息';
  }
  $('statBreaks').textContent = st.breaks_today;
  $('statStreak').textContent = st.streak;
}

function applyConfig(c) {
  $('work').value = c.work_minutes;
  $('break').value = c.break_minutes;
  $('enabled').checked = c.enabled;
  $('logLevel').value = c.log_level;
  $('autostartDaemon').checked = c.autostart_daemon;
  $('animation').checked = c.animation_enabled;
  $('mascot').value = c.mascot || 'cat';
  $('messages').value = c.messages || '';
  $('autostartTray').checked = c.autostart_tray;
  $('pwOld').value = ''; $('pwNew').value = ''; $('pwConfirm').value = '';
  $('pwError').textContent = '';
}

function receive(p) {
  switch (p.event) {
    case 'open':
      applyConfig(p.config);
      updateStatus(p.status);
      closePrompt();
      break;
    case 'prompt':
      openPrompt(p.purpose);
      break;
    case 'prompt-error':
      $('stopError').textContent = p.error || '口令错误';
      break;
    case 'prompt-done':
      closePrompt();
      send({ cmd: 'close' });
      break;
    case 'status':
      updateStatus(p.status);
      break;
    case 'config':
      var c = p.config;
      $('enabled').checked = c.enabled;
      $('autostartDaemon').checked = c.autostart_daemon;
      $('autostartTray').checked = c.autostart_tray;
      break;
    case 'saved':
      $('pwOld').value = ''; $('pwNew').value = ''; $('pwConfirm').value = '';
      $('pwError').textContent = '';
      toast('设置已保存');
      break;
    case 'save-error':
      selectTab('security');
      $('pwError').textContent = p.error || '保存失败';
      break;
  }
}
window.__stretchlock = { receive: receive };

function collectDraft() {
  return {
    work_minutes: parseInt($('work').value, 10) || 0,
    break_minutes: parseInt($('break').value, 10) || 0,
    enabled: $('enabled').checked,
    log_level: $('logLevel').value,
    autostart_daemon: $('autostartDaemon').checked,
    autostart_tray: $('autostartTray').checked,
    animation_enabled: $('animation').checked,
    mascot: $('mascot').value,
    messages: $('messages').value
  };
}

function collectPassphraseChange() {
  var old_ = $('pwOld').value, nw = $('pwNew').value, cf = $('pwConfirm').value;
  if (!old_ && !nw && !cf) { return null; }
  return { old: old_, new: nw, confirm: cf };
}

/* client-side validation; the host re-verifies the old passphrase (PBKDF2). */
function passphraseProblem() {
  var nw = $('pwNew').value, cf = $('pwConfirm').value;
  var touched = $('pwOld').value || nw || cf;
  if (!touched) { return null; }
  if (!nw) { return '新口令不能为空'; }
  if (nw !== cf) { return '两次输入的新口令不一致'; }
  return null;
}

var toastTimer = null;
function toast(msg) {
  var t = $('toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(function () { t.classList.remove('show'); }, 2200);
}

/* tabs */
function selectTab(name) {
  document.querySelectorAll('#tabs button').forEach(function (b) {
    b.setAttribute('aria-selected', String(b.dataset.tab === name));
  });
  ['general', 'security', 'appearance', 'advanced'].forEach(function (n) {
    $('tab-' + n).classList.toggle('hidden', n !== name);
  });
}
document.querySelectorAll('#tabs button').forEach(function (btn) {
  btn.addEventListener('click', function () { selectTab(btn.dataset.tab); });
});

/* steppers */
document.querySelectorAll('.stepper button').forEach(function (btn) {
  btn.addEventListener('click', function () {
    var input = $(btn.dataset.for);
    var min = parseInt(input.min, 10), max = parseInt(input.max, 10);
    var value = (parseInt(input.value, 10) || min) + parseInt(btn.dataset.step, 10);
    input.value = Math.min(max, Math.max(min, value));
  });
});

/* title bar */
$('closeBtn').addEventListener('click', function () { send({ cmd: 'close' }); });
$('titlebar').addEventListener('mousedown', function (e) {
  if (e.button === 0 && e.target.id !== 'closeBtn') { send({ cmd: 'drag' }); }
});

/* save / change passphrase */
$('saveBtn').addEventListener('click', function () {
  var problem = passphraseProblem();
  if (problem) {
    selectTab('security');
    $('pwError').textContent = problem;
    return;
  }
  send({ cmd: 'save', config: collectDraft(), passphrase_change: collectPassphraseChange() });
});
$('changePw').addEventListener('click', function () {
  var problem = passphraseProblem();
  if (problem) { $('pwError').textContent = problem; return; }
  var change = collectPassphraseChange();
  if (!change) { $('pwError').textContent = '请先填写口令字段'; return; }
  send({ cmd: 'save', config: collectDraft(), passphrase_change: change });
});

/* passphrase gate (covers stop / disable / quit / settings) */
var promptPurpose = 'stop';
var PROMPT_TEXT = {
  disable: { title: '停用计时器', desc: '停用后不再提醒休息，直到重新启用。请输入口令确认。' },
  settings: { title: '进入设置', desc: '请输入口令以打开设置。' },
  quit: { title: '退出托盘', desc: '退出托盘后服务仍在后台运行。请输入口令确认。' },
  stop: { title: '停止 StretchLock 服务', desc: '停止后休息提醒将暂停，直到重新启动服务。请输入口令确认。' }
};
function openPrompt(purpose) {
  promptPurpose = purpose;
  var t = PROMPT_TEXT[purpose] || PROMPT_TEXT.stop;
  $('promptTitle').textContent = t.title;
  $('promptDesc').textContent = t.desc;
  $('stopPassphrase').value = '';
  $('stopError').textContent = '';
  $('stopModal').classList.add('open');
  $('stopPassphrase').focus();
}
function closePrompt() { $('stopModal').classList.remove('open'); }
$('stopBtn').addEventListener('click', function () { openPrompt('stop'); });
$('stopCancel').addEventListener('click', closePrompt);
$('promptConfirm').addEventListener('click', function () {
  var pw = $('stopPassphrase').value;
  if (!pw) { $('stopError').textContent = '请输入口令'; return; }
  send({ cmd: 'prompt', purpose: promptPurpose, passphrase: pw });
});
document.addEventListener('keydown', function (e) {
  if (e.key === 'Escape') { closePrompt(); }
  if (e.key === 'Enter' && $('stopModal').classList.contains('open')) {
    $('promptConfirm').click();
  }
});
$('stopModal').addEventListener('mousedown', function (e) {
  if (e.target === $('stopModal')) { closePrompt(); }
});

send({ cmd: 'ready' });
</script>
</body>
</html>"#;
