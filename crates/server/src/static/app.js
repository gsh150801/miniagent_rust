// ── State ──
let ws = null;
let wsConnected = false;
let tasks = {};
let currentTaskId = null;
let uploadedFiles = [];
let isStreaming = false;
let isRunning = false;   // P3: a pipeline is actively executing (steer window open)
// Skills state
let skills = [];                    // [{name, description, triggers, tools_needed, ...}]
let selectedSkills = new Set();     // user-selected skill names
let skillPanelOpen = true;
// Right panel state
let activeRightTab = 'progress';      // 'progress' | 'files'
let rightPanelOpen = true;
let fileTree = [];                     // current task's file tree
let previewing = null;                 // {path, name, ext, size}
// Progress state
let taskStartTime = null;              // Date.now() when a run starts
let elapsedTimer = null;
let currentPlan = null;                // {workflow, stages} for the active task
let stageStatus = {};                  // {stageName: 'running'|'completed'|'failed'}
// Stage 3: real-time agent activity stream
let activityFeed = [];                 // [{kind:'tool_start'|'tool_end'|'skill'|'subtask'|'run', ...}]
let activityStats = { tools: 0, toolErrors: 0, skills: 0, subtasks: 0, iterations: 0 };
// Subtask todo list + live pipeline phases (loop / research modes).
// subtasks: task_id -> {id,title,role,difficulty,status,output,error,tokens,reused,ts}
// phases: ordered [{name,label,status,summary,ts}] from progress events that
// are NOT part of the plan's stage list (loop phases, research phases).
let subtasks = {};
let subtaskOrder = [];
let phases = [];

// ── WebSocket ──
function connect() {
  setConnState('connecting');
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  ws = new WebSocket(`${proto}://${location.host}/ws/chat`);
  ws.onopen = () => { setConnState('connected'); showToast('Connected', false, 'success'); loadTasks(); loadSkills(); loadSettings(); };
  ws.onmessage = (e) => {
    try { handleMsg(JSON.parse(e.data)); }
    catch(err) { handleMsg({ type: 'stream', text: e.data }); }
  };
  ws.onclose = () => { setConnState('disconnected'); showToast('Disconnected', true); setTimeout(connect, 3000); };
  ws.onerror = () => {};
}

function setConnState(s) {
  wsConnected = (s === 'connected');
  const bar = document.getElementById('connBar');
  if (bar) bar.className = 'conn-bar ' + (s === 'connected' ? 'connected' : 'disconnected');
  const txt = document.getElementById('connText');
  if (txt) txt.textContent = s === 'connected' ? '已连接' : (s === 'connecting' ? '连接中...' : '已断开');
}

// Local cache of known task statuses, mirroring what the backend has reported.
// Keyed by task_id; populated from `task_started`, `progress`, `plan`,
// `complete`, `error` events. The middle (task list) panel reads from
// `tasks[task_id].status`; the right (progress) panel reads from
// `stageStatus`. Keeping this mirror lets us sync the middle panel without
// round-tripping the whole task list on every WebSocket event.
function syncTaskMeta(taskId, patch) {
  if (!taskId || !patch) return;
  const cur = tasks[taskId];
  if (!cur) return;
  Object.assign(cur, patch);
  renderTaskList();
}

function handleMsg(msg) {
  switch(msg.type) {
    case 'status': addSystemMsg(msg.message); break;
    case 'task_started':
      currentTaskId = msg.task_id;
      isRunning = true;
      // Backend may have attached a status/brief snapshot — apply it so the
      // middle panel list-item doesn't briefly show "—" before the next list
      // refresh.
      if (msg.task_id) syncTaskMeta(msg.task_id, {
        status: msg.status || 'running',
        brief: msg.brief || tasks[msg.task_id]?.brief,
      });
      renderTaskList();
      break;
    case 'plan':
      // Only apply plans for the currently selected task — switching to a
      // historical task mid-run shouldn't repaint its panel with a new run.
      if (msg.task_id && currentTaskId && msg.task_id !== currentTaskId) break;
      currentPlan = { workflow: msg.workflow, stages: msg.stages };
      // If progress events arrived before the plan (workflow mode), drop any
      // "phase" entries that are actually plan stages so they don't render
      // twice (Pipeline Phases + Workflow Stages).
      if (Array.isArray(msg.stages) && msg.stages.length) {
        const names = new Set(msg.stages.map(s => s.name));
        phases = phases.filter(p => !names.has(p.name));
      }
      // Loop pipeline: the "stages" are actually the decomposed subtask
      // units (T1..Tn). Seed the todo list so the right panel shows the
      // full checklist before the first `task` progress event arrives.
      if (msg.workflow === 'loop_pipeline' && Array.isArray(msg.stages)) {
        for (const s of msg.stages) {
          if (!subtasks[s.name]) {
            subtasks[s.name] = { id: s.name, title: s.description || s.name,
              role: s.handler || '', status: 'pending' };
            subtaskOrder.push(s.name);
          }
        }
      }
      showPlan(msg.workflow, msg.stages);
      renderProgressView();
      // Middle panel: a "running" task now has a plan → make sure the status
      // pill in the list reflects that.
      syncTaskMeta(msg.task_id, { status: 'running' });
      break;
    case 'progress':
      // Same task filter as plan: ignore progress for other tasks.
      if (msg.task_id && currentTaskId && msg.task_id !== currentTaskId) break;
      handleProgressMsg(msg);
      break;
    case 'ask':
      // 双向 ws：后端反问用户，渲染输入框/选项卡
      renderAsk(msg.task_id, msg.question, msg.options || []);
      break;
    case 'stage_output': showStageOutput(msg.stage, msg.summary); break;
    case 'stream': appendStream(msg.text); break;
    case 'agent_event': handleAgentEvent(msg); break;
    case 'complete':
      stopElapsed();
      stageStatus = {};
      liveToolOps.clear();
      // Failure completions carry status:"failed" with no files — don't mark
      // the task completed (the list would show a bogus ✓ until refresh).
      if (msg.status === 'failed') {
        finishStreamError('任务执行失败（详见服务器日志 / project.json 事件日志）');
        syncTaskMeta(msg.task_id, { status: 'failed' });
      } else {
        finishStream(msg.task_id, msg.files);
        syncTaskMeta(msg.task_id, { status: 'completed' });
      }
      renderProgressView();
      break;
    case 'error':
      stopElapsed();
      finishStreamError(msg.message);
      syncTaskMeta(msg.task_id, { status: 'failed' });
      renderProgressView();
      break;
    case 'tasks':
      tasks = msg.tasks;
      renderTaskList();
      break;
    case 'hypotheses':
      // 结构化假说/辩论卡片（research 管线）：渲染真正的卡片而非 markdown。
      // 与 plan/progress 相同的任务过滤——切到历史任务时不被新事件污染。
      if (msg.task_id && currentTaskId && msg.task_id !== currentTaskId) break;
      if (Array.isArray(msg.hypotheses) && msg.hypotheses.length) {
        showHypothesisCards(msg.hypotheses, null, msg.cross);
      }
      break;
    case 'validation':
      // 验证计划结构化卡片（目标3）：数据分析任务 + 湿实验方案。
      if (msg.task_id && currentTaskId && msg.task_id !== currentTaskId) break;
      if (Array.isArray(msg.plans) && msg.plans.length) {
        showValidationCards(msg.plans);
      }
      break;
    case 'task_messages': renderHistory(msg); break;
  }
}

// ── Stage 3: handle fine-grained agent events ──
// AgentEvent is serialized with #[serde(tag="type")] so msg.event.type is the
// variant name in snake_case: tool_call_requested, tool_call_completed,
// skill_invoked, subtask_started, subtask_completed, run_started, run_completed.
//
// The WS envelope `{type:"agent_event", event:{...}, task_id}` may carry a
// task_id; if so, only apply events that match the currently selected task
// so switching tasks doesn't bleed events across.
// ── P-前端改进：实时操作卡（操作 + 参数收起 / 结果展开）──────────
const liveToolOps = new Map(); // call_id -> {el, body, tool}

// 操作中文名 + 参数行（收起态显示"操作 + 关键参数"）
function toolOpDisplay(tool, input) {
  const val = (k) => {
    if (!input) return '';
    const v = input[k] ?? input[k === 'command' ? 'command' : k];
    return typeof v === 'string' ? v : (v ? JSON.stringify(v) : '');
  };
  switch (tool) {
    case 'read':    return { icon: '&#128196;', label: '读取文件', param: val('path') || val('file') };
    case 'write':   return { icon: '&#9997;',   label: '写入文件', param: val('path') || val('file') };
    case 'edit':    return { icon: '&#9997;',   label: '编辑文件', param: val('path') || val('file') };
    case 'bash':    return { icon: '&#128187;', label: '执行命令', param: val('command') };
    case 'web_search':
    case 'search':  return { icon: '&#128269;', label: '网络搜索', param: val('query') };
    case 'web_fetch': return { icon: '&#127760;', label: '抓取网页', param: val('url') };
    case 'pubmed_search': return { icon: '&#128269;', label: 'PubMed 检索', param: val('query') };
    case 'citation_check': return { icon: '&#128269;', label: '引用核验', param: '报告全文' };
    case 'glob':    return { icon: '&#128269;', label: '文件匹配', param: val('pattern') };
    case 'grep':    return { icon: '&#128269;', label: '内容搜索', param: val('pattern') };
    default:        return { icon: '&#128295;', label: tool, param: summarizeInput(input) };
  }
}

function appendToolOpCard(callId, tool, input) {
  const meta = toolOpDisplay(tool, input);
  hideWelcome();
  const inner = getInner();
  const card = document.createElement('div');
  card.className = 'op-card collapsed';
  card.id = 'op_' + callId.replace(/[^a-zA-Z0-9]/g, '');
  const paramLine = meta.param
    ? `<span class="op-param">${escHtml(meta.param.slice(0, 120))}</span>`
    : '';
  card.innerHTML = `<div class="op-head">
      <span class="op-chev">&#9654;</span>
      <span class="op-icon">${meta.icon}</span>
      <span class="op-label">${escHtml(meta.label)}</span>
      ${paramLine}
      <span class="op-status" id="${card.id}_status">&#8987;</span>
    </div><div class="op-body" style="display:none"></div>`;
  card.querySelector('.op-head').addEventListener('click', () => {
    const body = card.querySelector('.op-body');
    body.style.display = body.style.display === 'none' ? '' : 'none';
    card.classList.toggle('collapsed');
    card.querySelector('.op-chev').innerHTML = card.classList.contains('collapsed') ? '&#9654;' : '&#9660;';
  });
  inner.appendChild(card);
  scrollBottom();
  return card;
}

function fillToolOpResult(callId, tool, output, durationMs, isError) {
  const card = document.getElementById('op_' + String(callId).replace(/[^a-zA-Z0-9]/g, ''));
  if (!card) return;
  const status = card.querySelector('.op-status');
  if (status) status.innerHTML = isError ? '&#9888;' : '&#10003;';
  if (durationMs) {
    const d = document.createElement('span');
    d.className = 'op-duration';
    d.textContent = (durationMs / 1000).toFixed(1) + 's';
    status?.parentNode?.appendChild(d);
  }
  const body = card.querySelector('.op-body');
  if (!body) return;
  const text = (output || '').slice(0, 4000);
  if (text.trim()) {
    body.innerHTML = `<div class="op-result">${escHtml(text)}${(output||'').length > 4000 ? '…[截断]' : ''}</div>`;
  } else {
    body.innerHTML = '<div class="op-result op-empty">（无输出）</div>';
  }
}

function handleAgentEvent(envelope) {
  const ev = envelope && envelope.event;
  if (!ev || !ev.type) return;
  // Always feed the agent event into the local activity feed + stats so the
  // feed reflects what the backend is emitting regardless of selection. But
  // only re-render the right panel when this event belongs to the currently
  // selected task — otherwise switching tasks would let unrelated events
  // bleed into the live view of the task the user is looking at.
  const matchesCurrent = !envelope.task_id || !currentTaskId || envelope.task_id === currentTaskId;
  ingestAgentEvent(ev, Date.now(), matchesCurrent);
  // P-前端改进：中间面板实时操作卡（操作+参数收起 / 结果展开）
  if (matchesCurrent && (ev.type === 'tool_call_requested' || ev.type === 'tool_call_completed')) {
    const cid = String(ev.call_id ?? '');
    if (ev.type === 'tool_call_requested') {
      if (!liveToolOps.has(cid)) {
        const el = appendToolOpCard(cid, ev.tool_name || 'unknown',
          typeof ev.input === 'string' ? null : ev.input);
        liveToolOps.set(cid, el);
      }
    } else {
      fillToolOpResult(cid, ev.tool_name || 'unknown', ev.output, ev.duration_ms, ev.is_error);
    }
  }
  // Middle panel: any agent event for a known task implies it's actively
  // running. Skip if we've already completed/failed it.
  if (envelope.task_id && tasks[envelope.task_id]) {
    const cur = tasks[envelope.task_id].status;
    if (cur !== 'completed' && cur !== 'failed') {
      tasks[envelope.task_id].status = 'running';
      renderTaskList();
    }
  }
}

// Summarize tool input for display — avoid dumping huge JSON.
function summarizeInput(input) {
  if (!input) return '';
  if (typeof input === 'string') return input.slice(0, 80);
  try {
    const s = JSON.stringify(input);
    return s.length > 80 ? s.slice(0, 77) + '...' : s;
  } catch { return ''; }
}
// The toggle button is now in chat-header (always visible). collapsed/expand
// are handled here directly; the floating sidebarReopen button is a fallback.
function toggleSidebar() {
  const sidebar = document.getElementById('sidebar');
  const wasCollapsed = sidebar.classList.contains('collapsed');
  if (wasCollapsed) {
    sidebar.classList.remove('collapsed');
    document.getElementById('sidebarReopen').style.display = 'none';
  } else {
    sidebar.classList.add('collapsed');
    sidebar.classList.remove('open');
    document.getElementById('sidebarReopen').style.display = 'flex';
  }
}
function expandSidebar() {
  const sidebar = document.getElementById('sidebar');
  sidebar.classList.remove('collapsed');
  document.getElementById('sidebarReopen').style.display = 'none';
}
function toggleSidebarMobile() {
  document.getElementById('sidebar').classList.toggle('open');
}

// ── Tasks ──
function loadTasks() {
  if (ws && ws.readyState === 1) ws.send(JSON.stringify({ type: 'list_tasks' }));
}

function filterTasks() { renderTaskList(); }

// ── Skills ──
async function loadSkills() {
  try {
    const resp = await fetch('/api/skills');
    if (resp.ok) {
      skills = await resp.json();
      renderSkillList();
    }
  } catch(err) { /* ignore — will retry on next connect */ }
}

function toggleSkillPanel() {
  skillPanelOpen = !skillPanelOpen;
  document.getElementById('skillPanel').style.display = skillPanelOpen ? '' : 'none';
  document.getElementById('skillChevron').innerHTML = skillPanelOpen ? '&#9660;' : '&#9654;';
}

function filterSkills() { renderSkillList(); }

function renderSkillList() {
  const el = document.getElementById('skillList');
  const count = document.getElementById('skillCount');
  if (!el) return;
  const query = (document.getElementById('skillSearch')?.value || '').trim().toLowerCase();

  const filtered = skills.filter(s => {
    if (!query) return true;
    const haystack = [s.name, s.description, (s.triggers||[]).join(' '), (s.tags||[]).join(' ')].join(' ').toLowerCase();
    return haystack.includes(query);
  });

  if (count) count.textContent = filtered.length;

  if (filtered.length === 0) {
    el.innerHTML = `<div class="mini-empty">No skills found</div>`;
    return;
  }

  el.innerHTML = filtered.map(s => {
    const selected = selectedSkills.has(s.name);
    const desc = (s.description || '').slice(0, 80);
    const icon = selected ? '&#9989;' : '&#11036;';
    return `<div class="skill-item ${selected ? 'selected' : ''}" data-name="${escHtml(s.name)}" onclick="toggleSkill('${escHtml(s.name)}')">
      <span class="skill-check">${icon}</span>
      <div class="skill-body">
        <div class="skill-name">${escHtml(s.name)}</div>
        ${desc ? `<div class="skill-desc">${escHtml(desc)}</div>` : ''}
      </div>
    </div>`;
  }).join('');
}

function toggleSkill(name) {
  if (selectedSkills.has(name)) {
    selectedSkills.delete(name);
  } else {
    selectedSkills.add(name);
  }
  renderSkillList();
  renderSkillChips();
}

function renderSkillChips() {
  const el = document.getElementById('skillChips');
  if (!el) return;
  el.innerHTML = '';
  for (const name of selectedSkills) {
    const chip = document.createElement('span');
    chip.className = 'file-chip skill-chip';
    chip.innerHTML = `&#128295; ${escHtml(name)} <span class="remove">&times;</span>`;
    chip.querySelector('.remove').addEventListener('click', () => {
      selectedSkills.delete(name);
      renderSkillList();
      renderSkillChips();
    });
    el.appendChild(chip);
  }
}

// Incremental render: reuse existing nodes by data-id, only update changed bits.
function renderTaskList() {
  const el = document.getElementById('taskList');
  const query = document.getElementById('searchInput').value.trim().toLowerCase();
  const sorted = Object.entries(tasks).sort((a,b) => {
    const da = new Date(a[1].created_at||0), db = new Date(b[1].created_at||0);
    return db - da;
  });

  // Collect visible ids in order
  const visible = [];
  for (const [id, t] of sorted) {
    const title = (t.brief || (t.prompt && t.prompt.slice(0,50)) || id);
    const prompt = (t.prompt || '');
    if (query && !title.toLowerCase().includes(query) && !prompt.toLowerCase().includes(query) && !id.toLowerCase().includes(query)) {
      continue;
    }
    visible.push({ id, t, title });
  }

  const existing = new Map();
  el.querySelectorAll('.task-item').forEach(n => existing.set(n.dataset.id, n));

  // Remove nodes no longer visible
  const visibleIds = new Set(visible.map(v => v.id));
  existing.forEach((node, id) => { if (!visibleIds.has(id)) node.remove(); });

  // Insert/update in order
  let prev = null;
  for (const { id, t, title } of visible) {
    const isActive = (id === currentTaskId);
    let node = existing.get(id);
    if (!node) {
      node = makeTaskNode(id, t, title, isActive);
    } else {
      updateTaskNode(node, t, title, isActive);
    }
    // Place in order
    if (prev) {
      if (prev.nextElementSibling !== node) prev.after(node);
    } else {
      if (el.firstChild !== node) el.prepend(node);
    }
    prev = node;
  }
}

/// 渲染 ask 消息：输入框 + 选项卡（需求1：ask 反问用户）
function renderAsk(taskId, question, options) {
  const card = document.createElement('div');
  card.className = 'msg-ask';

  const q = document.createElement('div');
  q.className = 'ask-question';
  q.textContent = '❓ ' + question;
  card.appendChild(q);

  // 回答后冻结为"问 → 答"记录（卡片保留在聊天流中，不删除）
  const freezeWithAnswer = (answer) => {
    const a = document.createElement('div');
    a.className = 'ask-answer';
    a.textContent = '➡️ ' + answer;
    card.appendChild(a);
    card.querySelectorAll('button, input').forEach(el => el.remove());
    card.classList.add('answered');
    scrollBottom();
  };

  // 选项按钮
  if (options.length > 0) {
    const optBox = document.createElement('div');
    optBox.className = 'ask-options';
    for (const opt of options) {
      const btn = document.createElement('button');
      btn.className = 'ask-option-btn';
      btn.textContent = opt;
      btn.addEventListener('click', () => {
        ws.send(JSON.stringify({ type: 'ask_reply', task_id: taskId, prompt: opt }));
        freezeWithAnswer(opt);
      });
      optBox.appendChild(btn);
    }
    card.appendChild(optBox);
  }

  // 文本输入框
  const inputBox = document.createElement('div');
  inputBox.className = 'ask-input-box';
  const input = document.createElement('input');
  input.className = 'ask-input';
  input.placeholder = 'Type your answer...';
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      const answer = input.value.trim();
      if (answer) {
        ws.send(JSON.stringify({ type: 'ask_reply', task_id: taskId, prompt: answer }));
        freezeWithAnswer(answer || '（未回答）');
      }
    }
  });
  inputBox.appendChild(input);

  const sendBtn = document.createElement('button');
  sendBtn.className = 'ask-send-btn';
  sendBtn.textContent = 'Reply';
  sendBtn.addEventListener('click', () => {
    const answer = input.value.trim();
    if (answer) {
      ws.send(JSON.stringify({ type: 'ask_reply', task_id: taskId, prompt: answer }));
      freezeWithAnswer(answer);
    }
  });
  inputBox.appendChild(sendBtn);
  card.appendChild(inputBox);

  insertCardBeforeResult(card);
  input.focus();
}

function makeTaskNode(id, t, title, isActive) {
  const div = document.createElement('div');
  div.className = 'task-item' + (isActive ? ' active' : '');
  div.dataset.id = id;
  div.addEventListener('click', (e) => { if (!e.target.classList.contains('btn-del') && !e.target.classList.contains('btn-trace')) selectTask(id); });
  const delBtn = document.createElement('button');
  delBtn.className = 'btn-del';
  delBtn.title = 'Delete';
  delBtn.innerHTML = '&times;';
  delBtn.addEventListener('click', (e) => { e.stopPropagation(); deleteTask(id, title); });
  const traceBtn = document.createElement('button');
  traceBtn.className = 'btn-trace';
  traceBtn.title = 'View trace';
  traceBtn.textContent = '📋';
  traceBtn.addEventListener('click', (e) => { e.stopPropagation(); viewTrace(id, title); });
  div.appendChild(traceBtn);
  div.appendChild(delBtn);
  const body = document.createElement('div');
  body.className = 'task-body';
  div.appendChild(body);
  fillTaskBody(body, t, title);
  return div;
}

function updateTaskNode(node, t, title, isActive) {
  node.classList.toggle('active', isActive);
  fillTaskBody(node.querySelector('.task-body'), t, title);
}

function fillTaskBody(body, t, title) {
  const time = t.created_at
    ? new Date(t.created_at).toLocaleString('zh-CN',{month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'})
    : '';
  const hasTrace = t.event_log && t.event_log.length > 0;
  body.innerHTML = `<div class="task-title">${escHtml(title)}</div>
    <div class="task-meta"><span class="status-dot ${t.status}"></span>${escHtml(t.status)}${time ? ' &middot; '+time : ''}${hasTrace ? ` &middot; <span class="trace-badge">${t.event_log.length} events</span>` : ''}</div>`;
}

/// 查看任务完整轨迹（需求2: 全链路可追溯）
async function viewTrace(taskId, title) {
  let trace;
  try {
    const resp = await fetch(`/api/trace/${taskId}`);
    trace = await resp.json();
  } catch (e) {
    showToast('Failed to load trace: ' + e.message, true, 'error');
    return;
  }
  if (trace.error) { showToast(trace.error, true, 'error'); return; }

  const events = trace.event_log || [];
  const overlay = document.createElement('div');
  overlay.className = 'confirm-overlay';
  overlay.style.alignItems = 'flex-start';

  // Two entry shapes: agent events ({ts, event:{type,...}}) and research
  // manifest events ({timestamp, kind, message} — surfaced by /api/trace
  // from project.json for research runs).
  const eventHtml = events.length === 0
    ? '<p class="mini-empty">No events recorded for this task.</p>'
    : events.map((entry, i) => {
        if (entry.kind || entry.timestamp) {
          // research manifest event
          const ts = entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString('zh-CN') : '';
          return `<div class="trace-event">
            <div class="trace-header"><span class="trace-kind">${escHtml(entry.kind || 'event')}</span><span class="trace-ts">${ts}</span></div>
            <div class="trace-manifest-msg">${escHtml(String(entry.message ?? ''))}</div>
          </div>`;
        }
        const ev = entry.event || {};
        const ts = entry.ts ? new Date(entry.ts).toLocaleTimeString('zh-CN') : '';
        const kind = ev.type || 'unknown';
        let detail = '';
        if (ev.tool_name) detail += `<div><b>Tool:</b> ${escHtml(ev.tool_name)}</div>`;
        if (ev.input) detail += `<div class="trace-input"><b>Input:</b> <pre>${escHtml(typeof ev.input === 'string' ? ev.input : JSON.stringify(ev.input, null, 2).slice(0, 500))}</pre></div>`;
        if (ev.output) detail += `<div class="trace-output"><b>Output:</b> <pre>${escHtml(typeof ev.output === 'string' ? ev.output.slice(0,500) : JSON.stringify(ev.output).slice(0,500))}</pre></div>`;
        if (ev.is_error) detail += `<div style="color:var(--red)">⚠ Error</div>`;
        if (ev.duration_ms) detail += `<div><b>Duration:</b> ${ev.duration_ms}ms</div>`;
        return `<div class="trace-event ${ev.is_error ? 'trace-error' : ''}">
          <div class="trace-header"><span class="trace-kind">${escHtml(kind)}</span><span class="trace-ts">${ts}</span></div>
          ${detail}
        </div>`;
      }).join('');

  overlay.innerHTML = `<div class="trace-modal">
    <div class="trace-title-bar">
      <h3>📋 Trace: ${escHtml(title)}</h3>
      <button class="btn-confirm no">Close</button>
    </div>
    <div class="trace-summary">
      <span>Status: ${escHtml(trace.status||'')}</span>
      <span>Events: ${events.length}</span>
      <span>Stages: ${(trace.stage_outputs||[]).length}</span>
      <span>Messages: ${trace.message_count||0}</span>
    </div>
    <div class="trace-events">${eventHtml}</div>
  </div>`;
  overlay.querySelector('.no').addEventListener('click', () => overlay.remove());
  overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
  document.body.appendChild(overlay);
}

function deleteTask(id, title) {
  const overlay = document.createElement('div');
  overlay.className = 'confirm-overlay';
  overlay.innerHTML = `<div class="confirm-box">
    <p>Delete task "${escHtml(title)}" ?</p>
    <div class="confirm-btns">
      <button class="btn-confirm no">Cancel</button>
      <button class="btn-confirm yes">Delete</button>
    </div></div>`;
  overlay.querySelector('.no').addEventListener('click', () => overlay.remove());
  overlay.querySelector('.yes').addEventListener('click', () => confirmDelete(id, overlay));
  document.body.appendChild(overlay);
}

async function confirmDelete(id, overlay) {
  overlay.remove();
  try {
    const resp = await fetch(`/api/tasks/${id}`, { method: 'DELETE' });
    if (resp.ok) {
      delete tasks[id];
      if (currentTaskId === id) newTask();
      renderTaskList();
      showToast('Task deleted', false, 'success');
    } else {
      showToast('Delete failed', true);
    }
  } catch(err) {
    showToast('Delete failed: ' + err.message, true);
  }
}

function selectTask(id) {
  currentTaskId = id;
  hideWelcome();
  renderTaskList();
  document.getElementById('chatTitle').textContent = tasks[id]?.brief || id;
  document.getElementById('sidebar').classList.remove('open');
  // Reset right-panel state for the new task BEFORE re-render so we don't
  // briefly flash the previous task's plan/activity.
  fileTree = [];
  stageStatus = {};
  currentPlan = null;
  activityFeed = [];
  activityStats = { tools: 0, toolErrors: 0, skills: 0, subtasks: 0, iterations: 0 };
  resultAnchor = null;
  resetSubtaskState();
  renderProgressView();
  renderFilesView();
  if (ws && ws.readyState === 1)
    ws.send(JSON.stringify({ type: 'get_task', task_id: id }));
}

// Replay an event_log array (each entry: {ts, event: {...AgentEvent}}) into
// the in-memory activity feed + stats so the right panel matches what the
// backend actually emitted.
function replayEventLog(eventLog) {
  if (!Array.isArray(eventLog) || eventLog.length === 0) return;
  for (const entry of eventLog) {
    const ev = (entry && entry.event) || entry;
    if (!ev || !ev.type) continue;
    const ts = entry.ts ? Date.parse(entry.ts) || Date.now() : Date.now();
    // Reuse the live handler logic, but suppress DOM re-render until the batch finishes.
    ingestAgentEvent(ev, ts, false);
  }
  renderProgressView();
}

// Push one agent event into the local feed/stats WITHOUT re-rendering.
// `reRender` toggles whether to call renderProgressView afterward.
function ingestAgentEvent(ev, ts, reRender) {
  if (!ev || !ev.type) return;
  let entry = null;
  switch (ev.type) {
    case 'run_started':
      activityStats.iterations++;
      entry = { kind: 'run', status: 'started', ts, text: 'Agent loop started' };
      break;
    case 'run_completed':
      entry = { kind: 'run', status: 'completed', ts, text: 'Agent loop completed',
        detail: ev.stop_reason ? `stop: ${ev.stop_reason}` : '' };
      break;
    case 'tool_call_requested':
      activityStats.tools++;
      entry = { kind: 'tool_start', ts, tool: ev.tool_name || 'unknown',
        input: summarizeInput(ev.input) };
      break;
    case 'tool_call_completed':
      if (ev.is_error) activityStats.toolErrors++;
      entry = { kind: 'tool_end', ts, tool: ev.tool_name || 'unknown',
        ok: !ev.is_error, duration: ev.duration_ms || 0,
        preview: (ev.output || '').slice(0, 120) };
      break;
    case 'skill_invoked':
      activityStats.skills++;
      entry = { kind: 'skill', ts, name: ev.skill_name || 'unknown', trigger: ev.trigger || '' };
      break;
    case 'subtask_started':
      activityStats.subtasks++;
      entry = { kind: 'subtask', status: 'started', ts, agent: ev.agent || '', step: ev.step || '' };
      break;
    case 'subtask_completed':
      entry = { kind: 'subtask', status: 'completed', ts, agent: ev.agent || '', step: ev.step || '', ok: ev.ok };
      break;
    default:
      return;
  }
  if (entry) {
    activityFeed.push(entry);
    if (activityFeed.length > 200) activityFeed = activityFeed.slice(-150);
    if (reRender !== false) renderProgressView();
  }
}

function renderHistory(msg) {
  const el = document.getElementById('messages');
  const pills = document.getElementById('stagePills');
  el.innerHTML = '<div class="messages-inner"></div>';
  pills.innerHTML = '';
  // The result-anchor belongs to the OLD inner — drop it so subsequent
  // upsertExecCard calls (trackPhase / restoreSubtask / live ops)
  // re-attach to the freshly-rendered inner instead of orphaning here.
  resultAnchor = null;
  resetSubtaskState();
  const inner = el.querySelector('.messages-inner');

  // Sync the middle panel status from the freshly-loaded task snapshot. This
  // is the source of truth after a task completes / fails while the user was
  // on another tab — without this the list-item stays at "running" forever.
  if (msg.task_id && msg.status && tasks[msg.task_id]) {
    tasks[msg.task_id].status = msg.status;
  }
  // P2: session goal_state (accumulated constraints) → right panel.
  currentGoalState = msg.goal_state || null;
  renderGoalState();

  if (msg.messages && msg.messages.length > 0) {
    for (const m of msg.messages) {
      if (m.role === 'user') {
        inner.appendChild(makeUserBubble(m.content));
      } else if (m.role === 'hypotheses') {
        try {
          const cards = JSON.parse(String(m.content ?? '[]'));
          if (Array.isArray(cards) && cards.length) showHypothesisCards(cards, inner, m.cross || null);
        } catch { /* legacy/corrupt entry — skip */ }
      } else if (m.role === 'validation') {
        try {
          const plans = JSON.parse(String(m.content ?? '[]'));
          if (Array.isArray(plans) && plans.length) showValidationCards(plans, inner);
        } catch { /* legacy/corrupt entry — skip */ }
      } else if (m.role === 'assistant') {
        const content = String(m.content ?? '');
        if (content.startsWith('❓')) {
          // 澄清反问（记忆机制 P3/P4：反问与回答在重绘后保留）
          const div = document.createElement('div');
          div.className = 'msg msg-ask';
          div.innerHTML = `<div class="msg-bubble">🔔 ${escHtml(content)}</div>`;
          inner.appendChild(div);
        } else {
          const div = document.createElement('div');
          div.className = 'msg msg-ai';
          div.innerHTML = `<div class="msg-bubble rich">${md(content)}</div>`;
          inner.appendChild(div);
        }
      }
    }
  } else {
    if (msg.prompt) inner.appendChild(makeUserBubble(msg.prompt));
    if (msg.response) {
      const div = document.createElement('div');
      div.className = 'msg msg-ai';
      div.innerHTML = `<div class="msg-bubble rich">${md(msg.response)}</div>`;
      inner.appendChild(div);
    }
  }

  // Restore plan + stage pills with status
  if (msg.plan && msg.plan.stages) {
    currentPlan = { workflow: msg.plan.workflow, stages: msg.plan.stages };
    showPlan(msg.plan.workflow, msg.plan.stages);
    // Only terminal tasks get a blanket pill status — a RUNNING task must
    // not show every stage green (reload mid-run painted 8/8 completed).
    if (msg.status === 'completed' || msg.status === 'failed') {
      for (const s of msg.plan.stages) {
        stageStatus[s.name] = msg.status;
        updateStagePill(s.name, msg.status);
      }
    } else {
      // Running/unknown: derive per-stage state from persisted stage_outputs.
      const done = new Set((msg.stage_outputs || []).map(o => o.stage));
      for (const s of msg.plan.stages) {
        if (done.has(s.name)) {
          stageStatus[s.name] = 'completed';
          updateStagePill(s.name, 'completed');
        }
      }
    }
  }

  // Restore stage output cards. Persisted per-subtask summaries
  // (stage === 'subtask') rebuild the todo list + expandable execution cards;
  // known loop/research pipeline phases go through trackPhase so the right
  // panel's Pipeline Phases section survives reloads; everything else renders
  // as the classic stage-output card.
  if (msg.stage_outputs && msg.stage_outputs.length > 0) {
    const planStageNames = new Set((currentPlan?.stages || []).map(s => s.name));
    for (const so of msg.stage_outputs) {
      if (so.stage === 'subtask' && so.summary) {
        restoreSubtask(so.summary);
      } else if (so.stage && so.summary && PIPELINE_PHASES[so.stage] && !planStageNames.has(so.stage)) {
        trackPhase(so.stage, 'completed', { summary: so.summary.response_preview || '' });
      } else if (so.stage && so.summary) {
        showStageOutput(so.stage, so.summary);
      }
    }
  }

  // Restore file tree
  if (msg.file_tree) {
    fileTree = msg.file_tree;
    renderFilesView();
  } else if (currentTaskId) {
    loadFileTree(currentTaskId);
  }

  // Restore activity feed + stats from the persisted event_log so the right
  // panel matches what the backend actually executed for this task.
  if (msg.event_log && msg.event_log.length > 0) {
    replayEventLog(msg.event_log);
  }

  if (!msg.messages || msg.messages.length === 0) {
    if (msg.status === 'running') { inner.appendChild(makeSysBubble('Task is still running...')); }
    else if (msg.status === 'failed') { inner.appendChild(makeSysBubble('Task failed.')); }
  }
  if (msg.files && msg.files.length) inner.appendChild(makeDownloads(msg.task_id, msg.files));
  renderProgressView();
  scrollBottom();
}

function newTask() {
  currentTaskId = null;
  const el = document.getElementById('messages');
  el.innerHTML = '';
  document.getElementById('stagePills').innerHTML = '';
  document.getElementById('chatTitle').textContent = '';
  fileTree = [];
  stageStatus = {};
  currentPlan = null;
  resetSubtaskState();
  renderProgressView();
  renderFilesView();
  showWelcome();
  renderTaskList();
  document.getElementById('sidebar').classList.remove('open');
}

function hideWelcome() {
  const w = document.getElementById('welcome');
  if (w) w.remove();
}
function showWelcome() {
  const el = document.getElementById('messages');
  if (!el.querySelector('.welcome')) {
    el.innerHTML = `<div class="welcome" id="welcome">
      <div class="welcome-icon">&#129302;</div>
      <h2>Miniagent</h2>
      <p>AI Agent with dynamic workflow planning, tool execution, and multi-stage research.</p>
      <div class="welcome-tips">
        <div class="welcome-tip" onclick="useTip('Summarize recent advances in CRISPR gene editing')">CRISPR Research</div>
        <div class="welcome-tip" onclick="useTip('Compare BERT and GPT architectures')">BERT vs GPT</div>
        <div class="welcome-tip" onclick="useTip('Analyze the pros and cons of Rust vs Go')">Rust vs Go</div>
      </div></div>`;
  }
}

function useTip(text) {
  document.getElementById('input').value = text;
  sendMessage();
}

// ── Messages ──
function getInner() {
  let inner = document.getElementById('messages').querySelector('.messages-inner');
  if (!inner) {
    const el = document.getElementById('messages');
    el.innerHTML = '<div class="messages-inner"></div>';
    inner = el.querySelector('.messages-inner');
  }
  return inner;
}

function makeUserBubble(text) {
  const div = document.createElement('div');
  div.className = 'msg msg-user';
  div.innerHTML = `<div class="msg-bubble">${escHtml(text)}</div>`;
  return div;
}
function makeSysBubble(text) {
  const div = document.createElement('div');
  div.className = 'msg msg-system';
  div.innerHTML = `<div class="msg-bubble">${escHtml(text)}</div>`;
  return div;
}

function addUserMsg(text) { hideWelcome(); getInner().appendChild(makeUserBubble(text)); scrollBottom(); }
function addSystemMsg(text) { getInner().appendChild(makeSysBubble(text)); scrollBottom(); }

// Streaming: throttled plain-text append during stream, markdown only at completion.
let streamEl = null, streamRaw = '', streamPending = '', streamFlushTimer = null;
// Anchor element that always stays at the end of the message list.
// Plan cards and stage-output cards are inserted BEFORE this anchor, so the
// final result (stream text) + file downloads always appear last.
let resultAnchor = null;
// P2: session goal_state (accumulated cross-turn constraints).
let currentGoalState = null;

function renderGoalState() {
  const el = document.getElementById('rpGoal');
  if (!el) return;
  if (!currentGoalState || !currentGoalState.constraints || !currentGoalState.constraints.length) {
    el.innerHTML = '';
    el.style.display = 'none';
    return;
  }
  const rows = currentGoalState.constraints.map(c =>
    `<div class="goal-row"><span class="goal-text">${escHtml(c.text)}</span><span class="goal-src">${escHtml(c.source)}</span></div>`).join('');
  el.style.display = '';
  el.innerHTML = `<div class="goal-card">
    <div class="goal-title">&#127919; 会话约束 <span class="goal-ver">v${currentGoalState.version || 1}</span></div>
    <div class="goal-obj">${escHtml(currentGoalState.objective || '')}</div>
    ${rows}</div>`;
}

function ensureResultAnchor() {
  const inner = getInner();
  if (resultAnchor && resultAnchor.parentNode === inner) return;
  resultAnchor = document.createElement('div');
  resultAnchor.className = 'result-anchor';
  resultAnchor.style.cssText = 'display:contents';
  inner.appendChild(resultAnchor);
}

// Insert a card element BEFORE the result anchor so execution details stay
// above the final answer.
function insertCardBeforeResult(card) {
  ensureResultAnchor();
  resultAnchor.parentNode.insertBefore(card, resultAnchor);
}
function startStream() {
  streamRaw = '';
  streamPending = '';
  streamEl = null;
  resultAnchor = null;   // fresh anchor for this task run
  isStreaming = true;
  startElapsed();
  const cancelBtn = document.getElementById('btnCancel');
  if (cancelBtn) cancelBtn.classList.add('show');
  document.getElementById('btnSend').disabled = true;
}
function ensureStreamEl() {
  if (streamEl) return;
  hideWelcome();
  ensureResultAnchor();
  const div = document.createElement('div');
  div.className = 'msg msg-ai';
  div.innerHTML = `<div class="msg-bubble rich"><span class="cursor"></span></div>`;
  // Insert AFTER the anchor so the final answer is always last.
  if (resultAnchor.nextSibling) {
    resultAnchor.parentNode.insertBefore(div, resultAnchor.nextSibling);
  } else {
    resultAnchor.parentNode.appendChild(div);
  }
  streamEl = div.querySelector('.msg-bubble');
  scrollBottom();
}
function flushStream() {
  streamFlushTimer = null;
  if (!streamEl || !streamPending) return;
  // Render pending text as plain text (escaped) to avoid per-token markdown cost.
  streamEl.textContent = streamRaw;
  streamPending = '';
  const cur = document.createElement('span');
  cur.className = 'cursor';
  streamEl.appendChild(cur);
  scrollBottom();
}
function appendStream(text) {
  ensureStreamEl();
  streamRaw += text;
  streamPending += text;
  if (!streamFlushTimer) streamFlushTimer = setTimeout(flushStream, 50);
}
function finishStream(taskId, files) {
  isStreaming = false;
  const cancelBtn = document.getElementById('btnCancel');
  if (cancelBtn) cancelBtn.classList.remove('show');
  document.getElementById('btnSend').disabled = false;
  if (streamFlushTimer) { clearTimeout(streamFlushTimer); streamFlushTimer = null; }
  if (streamEl) {
    streamEl.innerHTML = md(streamRaw);
    enhanceRichBody(streamEl);
  } else if (streamRaw) {
    ensureStreamEl();
    streamEl.innerHTML = md(streamRaw);
    enhanceRichBody(streamEl);
  }
  streamEl = null; streamRaw = ''; streamPending = '';
  // Reset the result anchor so the next task starts fresh.
  resultAnchor = null;
  if (taskId) {
    currentTaskId = taskId;
    isRunning = false;   // P3: run finished — steer window closes
    if (files && files.length) getInner().appendChild(makeDownloads(taskId, files));
    loadTasks();
    loadFileTree(taskId);   // refresh directory after completion
  }
}
function finishStreamError(message) {
  isStreaming = false;
  const cancelBtn = document.getElementById('btnCancel');
  if (cancelBtn) cancelBtn.classList.remove('show');
  document.getElementById('btnSend').disabled = false;
  if (streamFlushTimer) { clearTimeout(streamFlushTimer); streamFlushTimer = null; }
  if (streamEl) streamEl.innerHTML = `<span style="color:var(--red)">${escHtml(message)}</span>`;
  streamEl = null; streamRaw = ''; streamPending = '';
}

function cancelTask() {
  let taskId = currentTaskId;
  // Fallback: if currentTaskId is null but streaming is active, try to find
  // the running task from the DOM (active task-item) or from tasks map.
  if (!taskId && isStreaming) {
    const activeEl = document.querySelector('.task-item.active');
    if (activeEl) taskId = activeEl.dataset.id;
  }
  if (!taskId) {
    showToast('No active task to cancel', true);
    return;
  }
  fetch('/api/cancel', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ task_id: taskId }),
  }).then(r => r.json()).then(d => {
    if (d.status === 'cancelled') showToast('Cancel requested', false);
    else showToast('Cancel failed: task not found', true);
  }).catch(e => showToast('Cancel error: ' + e.message, true));
}

function makeDownloads(taskId, files) {
  const div = document.createElement('div');
  div.className = 'msg msg-ai';
  const badges = files.map(f =>
    `<a class="file-download" href="/api/download/${taskId}/${encodeURIComponent(f)}" download><span class="icon">&#128196;</span>${escHtml(f.split('/').pop())}</a>`
  ).join('');
  div.innerHTML = `<div class="msg-bubble">${badges}</div>`;
  return div;
}

// ── Elapsed timer ──
function startElapsed() {
  taskStartTime = Date.now();
  stopElapsed();
  elapsedTimer = setInterval(renderProgressView, 1000);
}
function stopElapsed() {
  if (elapsedTimer) { clearInterval(elapsedTimer); elapsedTimer = null; }
}
function elapsedStr() {
  if (!taskStartTime) return '—';
  const s = Math.floor((Date.now() - taskStartTime) / 1000);
  if (s < 60) return s + 's';
  const m = Math.floor(s / 60), r = s % 60;
  return `${m}m ${r}s`;
}

// ── Stages ──
function showPlan(workflow, stages) {
  const el = document.getElementById('stagePills');
  el.innerHTML = '';
  for (const s of stages) {
    const pill = document.createElement('span');
    pill.className = 'stage-pill';
    pill.id = 'stage-' + s.name;
    pill.textContent = s.name;
    el.appendChild(pill);
  }

  hideWelcome();
  // 阶段卡片不再插入聊天流（用户反馈：探索/澄清/规划/执行/评估等
  // 阶段卡片是噪音）。右侧 Progress 视图仍完整展示计划与状态。
  return;
  const card = document.createElement('div');
  card.className = 'exec-panel';

  const typeIcons = { research: '&#128300;', analysis: '&#128202;', writing: '&#9997;', coding: '&#128187;', default: '&#9881;' };
  const typeIcon = typeIcons[workflow] || typeIcons.default;

  let stagesHtml = stages.map((s, i) => {
    const handlerIcon = s.handler === 'agent' ? '&#129302;'
      : s.handler === 'synthesizer' ? '&#9997;'
      : s.handler === 'critic' ? '&#128270;'
      : '&#9881;';
    const tierBadge = s.tier ? `<span class="tag-mini">${escHtml(s.tier)}</span>` : '';
    let descHtml = '';
    if (s.description) {
      descHtml = `<div class="pl-desc">${escHtml(s.description)}</div>`;
    }
    let subTasksHtml = '';
    if (s.sub_tasks && s.sub_tasks.length > 0) {
      const items = s.sub_tasks.map(t => `<li>${escHtml(t)}</li>`).join('');
      subTasksHtml = `<ul class="pl-sublist">${items}</ul>`;
    }
    let toolsHtml = '';
    if (s.tools && s.tools.length > 0) {
      const badges = s.tools.map(t => `<span class="tag-mini">${escHtml(t)}</span>`).join('');
      toolsHtml = `<div class="pl-tools">${badges}</div>`;
    }
    const arrow = i < stages.length - 1
      ? '<div class="pl-arrow">&#8595;</div>'
      : '';
    return `<div class="pl-stage">
      <div class="pl-head">
        <span class="pl-stage-icon">${handlerIcon}</span>
        <span class="pl-name">${escHtml(s.name)}</span>
        ${tierBadge}
        <span class="pl-handler">${escHtml(s.handler)}</span>
      </div>
      ${descHtml}${subTasksHtml}${toolsHtml}
    </div>${arrow}`;
  }).join('');

  card.innerHTML = `<div class="exec-card">
    <div class="exec-card-header">
      <span class="icon">${typeIcon}</span> Task Plan
      <span class="meta-note" style="margin-left:auto">${escHtml(workflow || 'auto')}</span>
    </div>
    <div style="margin-top:6px">${stagesHtml}</div>
  </div>`;
  insertCardBeforeResult(card);
  scrollBottom();
}
function updateStagePill(name, status) {
  const pill = document.getElementById('stage-' + name);
  if (pill) pill.className = 'stage-pill ' + status;
}
function toggleToolDetail(uid) {
  const detail = document.getElementById(uid + '_detail');
  const toggle = document.getElementById(uid + '_toggle');
  const preview = document.getElementById(uid + '_preview');
  if (!detail) return;
  const shown = detail.style.display !== 'none';
  detail.style.display = shown ? 'none' : 'block';
  if (toggle) toggle.innerHTML = shown ? '&#9660;' : '&#9650;';
  if (preview) preview.style.display = shown ? '' : 'none';
  // Do NOT scroll — preserve user's reading position when expanding/collapsing.
}

// Aggregate tool entries into activity stat badges.
function renderActivityStats(toolEntries) {
  if (!toolEntries || toolEntries.length === 0) return '';
  const counts = {};
  for (const e of toolEntries) {
    const n = e.name || 'unknown';
    counts[n] = (counts[n] || 0) + 1;
  }
  const TOOL_ICONS = { read:'&#128196;', write:'&#9997;', edit:'&#9997;', bash:'&#128187;', glob:'&#128269;', grep:'&#128269;', web_search:'&#128269;', web_fetch:'&#127760;', pubmed_search:'&#128269;' };
  const badges = Object.entries(counts).map(([name, n]) => {
    const icon = TOOL_ICONS[name] || '&#128295;';
    return `<span class="stat-badge"><span>${icon}</span> ${escHtml(name)} <strong>${n}</strong></span>`;
  }).join('');
  return `<div class="activity-stats">${badges}</div>`;
}

function showStageOutput(stage, summary) {
  // 阶段输出卡片不再插入聊天流（用户反馈）。每轮的具体操作（读/写/
  // 编辑/搜索/命令等）由 agent_event 的实时操作卡逐条展示。
  return;
  if (!summary) return;

  const icon = stage.includes('research') || stage.includes('agent') ? '&#128269;'
    : stage.includes('critic') || stage.includes('critique') ? '&#128270;'
    : stage.includes('synth') ? '&#9997;'
    : '&#9881;';

  const toolCount = summary.tool_count || 0;
  const tokensIn = summary.tokens_in || 0;
  const tokensOut = summary.tokens_out || 0;
  const toolEntries = summary.tool_entries || [];

  const headerCard = document.createElement('div');
  headerCard.className = 'exec-panel';
  let headerHtml = `<div class="exec-card"><div class="exec-card-header">`;
  headerHtml += `<span class="icon">${icon}</span> ${escHtml(stage)} complete`;
  if (tokensIn || tokensOut) {
    headerHtml += ` <span class="meta-note">${(tokensIn/1000).toFixed(1)}k in / ${(tokensOut/1000).toFixed(1)}k out</span>`;
  }
  if (toolCount) {
    headerHtml += ` <span class="meta-note">&#128295; ${toolCount} tool calls</span>`;
  }
  headerHtml += `</div>`;
  if (summary.response_preview) headerHtml += `<div class="exec-preview">${escHtml(summary.response_preview)}...</div>`;
  if (summary.critique_preview) headerHtml += `<div class="exec-preview">${escHtml(summary.critique_preview)}...</div>`;
  // Aggregate activity stats
  const statsHtml = renderActivityStats(toolEntries);
  if (statsHtml) headerHtml += statsHtml;
  headerHtml += `</div>`;
  headerCard.innerHTML = headerHtml;
  insertCardBeforeResult(headerCard);

  for (const entry of toolEntries) {
    const toolCard = document.createElement('div');
    toolCard.className = 'exec-panel';
    const nameStr = entry.name || 'unknown';
    const isSearch = nameStr.includes('search') || nameStr.includes('pubmed') || nameStr.includes('tavily');
    const isFetch = nameStr.includes('fetch');
    const nameIcon = isSearch ? '&#128269;'
      : isFetch ? '&#127760;'
      : nameStr.includes('bash') || nameStr.includes('exec') ? '&#128187;'
      : nameStr.includes('read') || nameStr.includes('glob') || nameStr.includes('grep') ? '&#128196;'
      : nameStr.includes('write') || nameStr.includes('edit') ? '&#9997;'
      : '&#128295;';
    const errorCls = entry.is_error ? ' is-error' : ' is-ok';
    const uid = 'tool_' + Math.random().toString(36).slice(2, 8);
    let toolHtml = `<div class="exec-card${errorCls}">`;
    toolHtml += `<div class="exec-card-header">`;
    toolHtml += `<span class="icon">${nameIcon}</span> <span class="mono-chip">${escHtml(nameStr)}</span>`;
    if (isFetch && entry.input_url) {
      toolHtml += ` <a href="${escHtml(entry.input_url)}" target="_blank" rel="noopener" class="link-inline">${escHtml(entry.input_url)}</a>`;
    } else if (entry.input_preview) {
      toolHtml += `<span class="meta-note" style="max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"> ${escHtml(entry.input_preview)}</span>`;
    }
    if (entry.result_expanded || (entry.urls && entry.urls.length > 0)) {
      toolHtml += ` <span class="tool-expand-toggle" onclick="toggleToolDetail('${uid}')" id="${uid}_toggle" title="Expand">&#9660;</span>`;
    }
    toolHtml += `</div>`;
    if (entry.result_preview) {
      const cls = entry.is_error ? 'exec-tool error' : 'exec-tool';
      toolHtml += `<div class="${cls}" id="${uid}_preview">${escHtml(entry.result_preview)}<div class="fade"></div></div>`;
    }
    const urls = entry.urls || [];
    if (urls.length > 0 || entry.result_expanded) {
      toolHtml += `<div class="tool-detail" id="${uid}_detail" style="display:none">`;
      if (urls.length > 0) {
        toolHtml += `<div class="tool-links">`;
        const expandedText = entry.result_expanded || '';
        for (const url of urls) {
          const urlIdx = expandedText.indexOf(url);
          let title = '';
          if (urlIdx > 0) {
            const before = expandedText.substring(Math.max(0, urlIdx - 200), urlIdx);
            const titleMatch = before.match(/(?:^|\n)\s*\d+\.\s*\*\*([^*]+)\*\*\s*$/);
            if (titleMatch) title = titleMatch[1].trim();
          }
          const displayTitle = title || url.replace(/^https?:\/\/(www\.)?/, '').split('/')[0];
          toolHtml += `<a class="tool-link" href="${escHtml(url)}" target="_blank" rel="noopener"><span class="tool-link-icon">&#128279;</span><span class="tool-link-text">${escHtml(displayTitle)}</span></a>`;
        }
        toolHtml += `</div>`;
      }
      if (entry.result_expanded) {
        toolHtml += `<div class="tool-expanded">${escHtml(entry.result_expanded)}</div>`;
      }
      toolHtml += `</div>`;
    }
    toolHtml += `</div>`;
    toolCard.innerHTML = toolHtml;
    insertCardBeforeResult(toolCard);
  }
  scrollBottom();
}

// ── Input ──
// 工作流模式选择下拉：workflow（默认，单 agent + 计划 + 反馈）/
// loop（迭代 explore→plan→dispatch→evaluate→repair）/
// debate（正方 vs 反方 → 裁判）
let currentMode = 'workflow';
const MODE_PLACEHOLDERS = {
  workflow: 'Ask anything...',
  loop: 'Loop pipeline: 迭代 explore→plan→dispatch→evaluate→repair',
  debate: '辩论模式：输入议题，正方 vs 反方 → 裁判...',
  research: '科研管线：输入疾病/研究问题，文献→知识图谱→致病机理假说→辩论→验证计划→数据分析 notebook',
};

const MODE_LABELS = {
  workflow: '⚙ Workflow',
  loop: '🔄 Loop',
  debate: '⚖ Debate',
  research: '🔬 Research',
};

function setMode(mode) {
  if (!MODE_PLACEHOLDERS[mode]) mode = 'workflow';
  currentMode = mode;
  const sel = document.getElementById('modeSelect');
  if (sel && sel.value !== mode) sel.value = mode;
  const input = document.getElementById('input');
  if (input) input.placeholder = MODE_PLACEHOLDERS[mode];
  const pill = document.getElementById('modePill');
  if (pill) pill.textContent = MODE_LABELS[mode] || mode;
}
// Initialize once at script load (init section also calls this for safety).
if (typeof document !== 'undefined') {
  document.addEventListener('DOMContentLoaded', () => {
    const sel = document.getElementById('modeSelect');
    if (sel) {
      sel.addEventListener('change', (e) => setMode(e.target.value));
      setMode(sel.value || 'workflow');
    }
  });
}

function sendMessage() {
  const input = document.getElementById('input');
  const text = input.value.trim();
  if (!text || isStreaming) return;
  addUserMsg(text);
  const fileIds = uploadedFiles.map(f => f.id);
  uploadedFiles = [];
  document.getElementById('fileChips').innerHTML = '';
  // P3 执行中转向：任务运行中（非流式输出间隙）再次发送 = 转向指令，
  // 不打断当前执行——server 入队，在下一个阶段边界生效。
  if (isRunning && currentTaskId) {
    ws.send(JSON.stringify({ type: 'steer', task_id: currentTaskId, prompt: text }));
    input.value = '';
    autoResize(input);
    return;
  }
  startStream();
  const payload = { type: 'run', prompt: text, files: fileIds };
  // Always send the chosen mode so the server can route to the right driver.
  // Unknown values fall through to the workflow default on the server.
  payload.mode = currentMode;
  if (currentTaskId) payload.task_id = currentTaskId;
  if (selectedSkills.size > 0) payload.skills = [...selectedSkills];
  ws.send(JSON.stringify(payload));
  input.value = '';
  autoResize(input);
}
function handleKey(e) {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }
}

// ── File Upload ──
document.getElementById('fileInput').addEventListener('change', async (e) => {
  for (const file of e.target.files) await uploadFile(file);
  e.target.value = '';
});
async function uploadFile(file) {
  const form = new FormData(); form.append('file', file);
  try {
    const resp = await fetch('/api/upload', { method: 'POST', body: form });
    const data = await resp.json();
    if (data.files) for (const f of data.files) { uploadedFiles.push(f); addFileChip(f); }
  } catch(err) { showToast('Upload failed: ' + err.message, true); }
}
function addFileChip(file) {
  const el = document.getElementById('fileChips');
  const chip = document.createElement('span');
  chip.className = 'file-chip';
  chip.dataset.id = file.id;
  chip.innerHTML = `${escHtml(file.name)} <span class="remove">&times;</span>`;
  chip.querySelector('.remove').addEventListener('click', () => removeFile(file.id, chip));
  el.appendChild(chip);
}
function removeFile(id, chip) {
  uploadedFiles = uploadedFiles.filter(f => f.id !== id);
  chip.remove();
}

// ── Drag & Drop ──
document.addEventListener('dragover', (e) => { e.preventDefault(); document.getElementById('dragOverlay').classList.add('active'); });
document.addEventListener('dragleave', (e) => { if (e.relatedTarget === null) document.getElementById('dragOverlay').classList.remove('active'); });
document.addEventListener('drop', async (e) => {
  e.preventDefault(); document.getElementById('dragOverlay').classList.remove('active');
  for (const file of e.dataTransfer.files) await uploadFile(file);
});

// ── Auto-resize ──
const inputEl = document.getElementById('input');
inputEl.addEventListener('input', () => autoResize(inputEl));
function autoResize(el) { el.style.height = 'auto'; el.style.height = Math.min(el.scrollHeight, 200) + 'px'; }

// ── Right panel: tabs ──
function toggleRightPanel() {
  const panel = document.getElementById('rightPanel');
  // If collapsed, the in-header button should expand it; if open, collapse it.
  if (panel.classList.contains('collapsed')) { expandRightPanel(); return; }
  panel.classList.add('collapsed');
  panel.classList.remove('open');
  document.getElementById('btnToggleRight').classList.remove('active');
  document.getElementById('rightReopen').style.display = 'flex';
}
function expandRightPanel() {
  const panel = document.getElementById('rightPanel');
  panel.classList.remove('collapsed');
  panel.classList.add('open');
  document.getElementById('btnToggleRight').classList.add('active');
  document.getElementById('rightReopen').style.display = 'none';
}
function switchRightTab(tab) {
  activeRightTab = tab;
  document.querySelectorAll('.rp-tab').forEach(t => t.classList.toggle('active', t.dataset.tab === tab));
  document.getElementById('rpProgress').style.display = (tab === 'progress') ? '' : 'none';
  document.getElementById('rpFiles').style.display = (tab === 'files') ? '' : 'none';
  if (tab === 'files' && currentTaskId && fileTree.length === 0) loadFileTree(currentTaskId);
  if (tab === 'progress') renderProgressView();
}

// ── Progress / subtask event handling ─────────────────────
// Chinese labels for the known loop-pipeline and research pipeline phases.
const PIPELINE_PHASES = {
  explore: '探索', clarify: '澄清', plan: '规划', dispatch: '执行',
  repair: '修复', evaluate: '评估',
  literature: '文献检索', kg: '知识图谱', prediction: '链路预测',
  hypotheses: '假说生成', debate: '证据辩论', validation: '验证计划',
  analysis: '数据分析', review: '报告审核',
};

// Route one `progress` WS envelope: per-subtask events (stage === 'task')
// drive the todo list + execution cards; everything else is a phase/stage
// status update.
function handleProgressMsg(msg) {
  const stage = msg.stage || '';
  const status = msg.status || '';
  const data = msg.data || null;

  if (stage === 'task') {
    if (data && data.task_id) {
      upsertSubtask({ ...data, status });
      stageStatus[data.task_id] = status;
      updateStagePill(data.task_id, status);
    }
    renderProgressView();
    return;
  }

  // Phase / workflow stage event.
  updateStagePill(stage, status);
  stageStatus[stage] = status;
  trackPhase(stage, status, data);
  syncTaskMeta(msg.task_id, { status: status === 'failed' ? 'failed' : 'running' });
  renderProgressView();
}

// Track a live pipeline phase (loop phases / research phases). Stages that
// belong to the current plan's stage list (workflow mode) are NOT tracked as
// phases — they render in the Workflow Stages tree instead.
function trackPhase(stage, status, data) {
  const planStageNames = new Set((currentPlan?.stages || []).map(s => s.name));
  const summary = data && typeof data.summary === 'string' ? data.summary : null;
  if (planStageNames.has(stage)) {
    // Research 模式：plan 内阶段也有丰富的执行详情（per-task 进度、三方
    // 裁决、GEO 下载……）。不进 Pipeline Phases 列表（避免与 Workflow
    // Stages 重复），但渲染中间面板执行卡——长时间运行不再是黑盒。
    if (currentPlan?.workflow === 'research' && (summary || status !== 'pending')) {
      upsertExecCard('ph-' + stage, {
        title: `${PIPELINE_PHASES[stage] || stage}`,
        role: stage,
        status,
        preview: mdPlain(summary || '', 140),
        bodyHtml: summary ? md(summary) : '',
        meta: '',
      });
    }
    return;
  }
  let p = phases.find(x => x.name === stage);
  if (!p) {
    p = { name: stage, label: PIPELINE_PHASES[stage] || stage, status, summary: null, ts: Date.now() };
    phases.push(p);
  }
  p.status = status;
  p.ts = Date.now();
  if (summary) p.summary = summary;
  // Execution summary card in the middle panel (expandable).
  upsertExecCard('ph-' + stage, {
    title: `${p.label || stage}`,
    role: stage,
    status,
    preview: mdPlain(summary || '', 140),
    bodyHtml: summary ? md(summary) : '',
    meta: '',
  });
}

// Insert-or-update one subtask in the todo list + its execution card.
function upsertSubtask(d) {
  const id = d.task_id;
  if (!subtasks[id]) {
    subtasks[id] = { id, status: 'pending' };
    subtaskOrder.push(id);
  }
  const st = subtasks[id];
  if (d.title) st.title = d.title;
  if (d.role) st.role = d.role;
  if (d.difficulty) st.difficulty = d.difficulty;
  st.status = d.status || st.status;
  st.reused = !!d.reused;
  if (d.output != null) st.output = d.output;
  if (d.error != null) st.error = d.error;
  if (d.tokens_used != null) st.tokens = d.tokens_used;
  st.ts = Date.now();
  upsertExecCard('st-' + id, {
    title: st.title || id,
    role: st.role,
    status: st.status,
    reused: st.reused,
    preview: mdPlain(st.status === 'failed' ? (st.error || '子任务执行失败') : (st.output || ''), 140),
    bodyHtml: st.status === 'failed'
      ? `<div class="sub-exec-error">${escHtml(st.error || '子任务执行失败')}</div>${st.output ? md(st.output) : ''}`
      : md(st.output || ''),
    meta: st.tokens ? `${st.tokens} tokens` : '',
  });
}

// Restore one persisted subtask summary (from task.stage_outputs) after a
// page reload / task switch.
function restoreSubtask(summary) {
  const id = summary.task_id || 'T' + (subtaskOrder.length + 1);
  if (!subtasks[id]) {
    subtasks[id] = { id, status: summary.status || 'completed' };
    subtaskOrder.push(id);
  }
  const st = subtasks[id];
  st.title = summary.title || st.title || id;
  st.role = summary.role || st.role || '';
  st.status = summary.status || st.status || 'completed';
  st.output = summary.response_preview || st.output || '';
  st.error = summary.error || null;
  st.tokens = summary.tokens_used || st.tokens || 0;
  st.reused = !!summary.reused;
  upsertExecCard('st-' + id, {
    title: st.title || id,
    role: st.role,
    status: st.status,
    reused: st.reused,
    preview: mdPlain(st.status === 'failed' ? (st.error || '') : (st.output || ''), 140),
    bodyHtml: st.status === 'failed'
      ? `<div class="sub-exec-error">${escHtml(st.error || '子任务执行失败')}</div>${st.output ? md(st.output) : ''}`
      : md(st.output || ''),
    meta: st.tokens ? `${st.tokens} tokens` : '',
  });
}

// ── Middle panel: expandable subtask execution summary cards ──
// Each card keeps its DOM node (id = key) so status updates replace content
// in place instead of appending duplicates.
function upsertExecCard(key, info) {
  let card = document.getElementById(key);
  const isNew = !card;
  if (isNew) {
    card = document.createElement('div');
    card.className = 'exec-panel';
    card.id = key;
    insertCardBeforeResult(card);
  }
  const icon = info.status === 'completed' ? '&#10003;'
    : info.status === 'failed' ? '&#10007;'
    : '<span class="check-spin"></span>';
  const cls = info.status === 'failed' ? 'is-error'
    : info.status === 'completed' ? 'is-ok' : '';
  // Running/failed cards start expanded; completed ones collapse to a
  // one-line preview (click to expand the full markdown-rendered output).
  const open = info.status !== 'completed';
  const preview = (info.preview || '').replace(/\s+/g, ' ').trim().slice(0, 140);
  const badgeTxt = info.status === 'completed' ? '完成'
    : info.status === 'failed' ? '失败' : '执行中';
  const badgeCls = info.status === 'completed' ? 'completed'
    : info.status === 'failed' ? 'failed' : 'running';
  card.innerHTML = `<div class="exec-card sub-exec ${cls}${open ? ' open' : ''}">
    <div class="exec-card-header sub-exec-head" onclick="toggleSubExec('${key}')">
      <span class="icon">${icon}</span>
      <span class="sub-exec-title">${escHtml(info.title || key)}</span>
      ${info.role ? `<span class="tag-mini">${escHtml(info.role)}</span>` : ''}
      ${info.reused ? '<span class="tag-mini">复用</span>' : ''}
      ${info.meta ? `<span class="meta-note">${escHtml(info.meta)}</span>` : ''}
      <span class="stage-badge ${badgeCls}">${badgeTxt}</span>
      <span class="tool-expand-toggle">&#9660;</span>
    </div>
    ${preview ? `<div class="sub-exec-preview">${escHtml(preview)}${(info.preview||'').length > preview.length ? '…' : ''}</div>` : ''}
    <div class="sub-exec-body">${info.bodyHtml || ''}</div>
  </div>`;
  if (isNew && isStreaming) scrollBottom();
}

function toggleSubExec(key) {
  const card = document.getElementById(key);
  if (!card) return;
  card.querySelector('.sub-exec')?.classList.toggle('open');
  // Do NOT scroll — preserve the user's reading position.
}

// Drop all per-task subtask/phase state (task switch / new task).
function resetSubtaskState() {
  subtasks = {};
  subtaskOrder = [];
  phases = [];
}

// One-line plain-text preview of a markdown blob (right-panel todo list):
// strips heading markers, emphasis, table pipes and links so it reads cleanly.
function mdPlain(s, max = 160) {
  if (!s) return '';
  const text = s
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/\|/g, ' ')
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/\*\*|__|`/g, '')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/^[-*]\s*\[[ xX]\]\s*/gm, '')
    .replace(/^[-*]\s+/gm, '')
    .replace(/^>\s?/gm, '')
    .replace(/\s+/g, ' ')
    .trim();
  return text.slice(0, max);
}

// ── Right panel: progress view ──
function renderProgressView() {
  const el = document.getElementById('rpProgress');
  if (!el) return;

  const stages = currentPlan?.stages || [];
  const isLoop = currentPlan?.workflow === 'loop_pipeline';
  const runningPhase = [...phases].reverse().find(p => p.status === 'running');

  // ── Header progress card ──
  // Prefer the REAL subtask todo list (loop mode); fall back to plan stages
  // (workflow), then to live phases (research without a plan).
  let done = 0, total = 0, running = false, pct = 0;
  let currentLabel = 'Idle', currentStatus = '';
  if (subtaskOrder.length) {
    total = subtaskOrder.length;
    done = subtaskOrder.filter(id => subtasks[id]?.status === 'completed').length;
    running = subtaskOrder.some(id => subtasks[id]?.status === 'running');
    pct = total ? Math.round((done / total) * 100) : 0;
  } else if (stages.length) {
    total = stages.length;
    done = stages.filter(s => stageStatus[s.name] === 'completed').length;
    running = stages.some(s => stageStatus[s.name] === 'running');
    pct = total ? Math.round((done / total) * 100) : 0;
  } else if (phases.length) {
    total = phases.length;
    done = phases.filter(p => p.status === 'completed').length;
    running = phases.some(p => p.status === 'running');
    pct = total ? Math.round((done / total) * 100) : 0;
  }
  if (runningPhase) {
    currentLabel = runningPhase.label || runningPhase.name;
    currentStatus = 'running';
  } else if (running) {
    const runningSub = subtaskOrder.map(id => subtasks[id]).find(s => s.status === 'running');
    currentLabel = runningSub ? (runningSub.title || runningSub.id) : '执行中';
    currentStatus = 'running';
  } else if (total > 0 && done === total) {
    currentLabel = 'Completed';
    currentStatus = 'completed';
  } else if (total > 0) {
    currentLabel = 'Pending';
  }
  const doneLabel = total ? `${done}/${total}` : '';

  let html = `<div class="progress-card">
    <div class="progress-stage">${escHtml(currentLabel)} <span class="stage-status ${currentStatus}">${currentStatus === 'running' ? 'running' : currentStatus === 'completed' ? 'done' : ''}</span></div>
    <div class="progress-bar-wrap"><div class="progress-bar" style="width:${pct}%"></div></div>
    <div class="progress-meta"><span>${doneLabel ? doneLabel + ' ' + (isLoop || (!stages.length && phases.length) ? '子任务' : 'stages') + ' &middot; ' : ''}${pct}%</span><span>&#9201; ${elapsedStr()}</span></div>
    <div class="activity-summary">
      <span class="act-stat" title="Tool calls">&#128295; ${activityStats.tools}</span>
      ${activityStats.toolErrors ? `<span class="act-stat err" title="Tool errors">&#9888; ${activityStats.toolErrors}</span>` : ''}
      <span class="act-stat" title="Skills">&#9881; ${activityStats.skills}</span>
      <span class="act-stat" title="Subtasks">&#128203; ${activityStats.subtasks}</span>
      <span class="act-stat" title="Agent iterations">&#128260; ${activityStats.iterations}</span>
    </div>
  </div>`;

  // ── Live pipeline phases (loop / research modes) ──
  if (phases.length) {
    html += `<div class="stree-section-title">Pipeline Phases</div>`;
    for (const p of phases) {
      const icon = p.status === 'completed' ? '&#10003;'
        : p.status === 'running' ? '<span class="check-spin"></span>'
        : p.status === 'failed' ? '&#10007;'
        : '&#9675;';
      html += `<div class="stree-stage ${p.status} ${p.summary ? 'expanded' : ''}">
        <div class="stree-stage-head" onclick="this.parentElement.classList.toggle('expanded')">
          <span class="check ${p.status}">${icon}</span>
          <span class="name">${escHtml(p.label || p.name)}</span>
          <span class="mono-chip">${escHtml(p.name)}</span>
          <span class="stage-badge ${p.status}">${p.status}</span>
          <span class="chevron">&#9654;</span>
        </div>
        ${p.summary ? `<div class="stree-stage-body"><div class="stree-stage-desc">${escHtml(p.summary)}</div></div>` : ''}
      </div>`;
    }
  }

  // ── Plan stages: workflow stages OR the loop-mode subtask todo list ──
  if (stages.length) {
    html += `<div class="stree-section-title">${isLoop ? 'Subtasks · Todo' : 'Workflow Stages'}</div>`;
    for (let si = 0; si < stages.length; si++) {
      const s = stages[si];
      // Loop mode: real per-subtask status from progress events. Workflow
      // mode: derive from the stage status as before.
      let status, subStatusNote = '';
      if (isLoop) {
        status = subtasks[s.name]?.status || 'pending';
        const st = subtasks[s.name];
        if (st && (st.output || st.error)) {
          subStatusNote = `<div class="stree-stage-desc">${escHtml(mdPlain(st.status === 'failed' ? '✗ ' + (st.error || '') : (st.output || '')))}</div>`;
        }
      } else {
        status = stageStatus[s.name] || 'pending';
      }
      const stageIcon = status === 'completed' ? '&#10003;'
        : status === 'running' ? '<span class="check-spin"></span>'
        : status === 'failed' ? '&#10007;'
        : '&#9675;';
      const stageCls = status;

      // Sub-task status (workflow mode only): derive from stage status.
      const subTasks = s.sub_tasks || [];
      let subItemsHtml = '';
      if (!isLoop) {
        for (let ti = 0; ti < subTasks.length; ti++) {
          let subStatus = 'pending';
          let subIcon = '&#9675;';
          if (status === 'completed') {
            subStatus = 'done'; subIcon = '&#10003;';
          } else if (status === 'failed') {
            subStatus = 'failed'; subIcon = '&#10007;';
          } else if (status === 'running') {
            if (ti === 0) { subStatus = 'active'; subIcon = '<span class="check-spin"></span>'; }
          }
          subItemsHtml += `<div class="stree-subtask ${subStatus}">
            <span class="check">${subIcon}</span>
            <span>${escHtml(subTasks[ti])}</span>
          </div>`;
        }
      }

      const toolBadges = (s.tools || []).map(t => `<span class="stree-tool">${escHtml(t)}</span>`).join('');
      const roleLabel = s.handler ? `<span class="role">${escHtml(s.handler)}</span>` : '';
      const descHtml = s.description ? `<div class="stree-stage-desc">${escHtml(s.description)}</div>` : '';
      const tierBadge = s.tier ? `<span class="stree-tier">${escHtml(s.tier)}</span>` : '';

      html += `<div class="stree-stage ${stageCls} expanded" data-stage="${escHtml(s.name)}">
        <div class="stree-stage-head" onclick="this.parentElement.classList.toggle('expanded')">
          <span class="check ${stageCls}">${stageIcon}</span>
          <span class="stage-idx">${si + 1}</span>
          <span class="name">${escHtml(s.name)}</span>
          ${roleLabel}${tierBadge}
          <span class="stage-badge ${stageCls}">${status}</span>
          <span class="chevron">&#9654;</span>
        </div>
        <div class="stree-stage-body">
          ${descHtml}
          ${subStatusNote}
          ${subItemsHtml}
          ${toolBadges ? `<div class="stree-tool-list">${toolBadges}</div>` : ''}
        </div>
      </div>`;
    }
  } else if (!phases.length) {
    html += `<div class="rp-empty"><div class="icon">&#128202;</div><div>No workflow yet. Run a task to see its stages.</div></div>`;
  }

  // Real-time activity feed (Stage 3)
  html += renderActivityFeedHtml();
  el.innerHTML = html;
}

// Render the real-time activity feed section.
function renderActivityFeedHtml() {
  if (activityFeed.length === 0) return '';
  let html = `<div data-section="activity"><div class="stree-section-title">Live Activity</div>`;
  // Show most recent first, cap at 30 items
  const items = activityFeed.slice(-30).reverse();
  for (const e of items) {
    const time = new Date(e.ts).toLocaleTimeString('en-US', {hour12:false, hour:'2-digit', minute:'2-digit', second:'2-digit'});
    html += renderActivityEntry(e, time);
  }
  html += '</div>';
  return html;
}

function renderActivityEntry(e, time) {
  const timeTag = `<span class="act-time">${time}</span>`;
  switch (e.kind) {
    case 'tool_start':
      return `<div class="act-entry tool-start">
        <span class="act-icon">&#128295;</span>
        <span class="act-body"><strong>${escHtml(e.tool)}</strong>${e.input ? ` <span class="act-detail">${escHtml(e.input)}</span>` : ''}</span>
        ${timeTag}
      </div>`;
    case 'tool_end':
      return `<div class="act-entry tool-end ${e.ok ? 'ok' : 'err'}">
        <span class="act-icon">${e.ok ? '&#10003;' : '&#10007;'}</span>
        <span class="act-body"><strong>${escHtml(e.tool)}</strong> ${e.ok ? 'completed' : 'failed'}${e.duration ? ` <span class="act-detail">${e.duration}ms</span>` : ''}${e.preview && !e.ok ? ` <span class="act-detail">${escHtml(e.preview)}</span>` : ''}</span>
        ${timeTag}
      </div>`;
    case 'skill':
      return `<div class="act-entry skill">
        <span class="act-icon">&#9881;</span>
        <span class="act-body">Skill: <strong>${escHtml(e.name)}</strong></span>
        ${timeTag}
      </div>`;
    case 'subtask':
      return `<div class="act-entry subtask ${e.status}">
        <span class="act-icon">${e.status === 'completed' ? (e.ok ? '&#10003;' : '&#10007;') : '&#9654;'}</span>
        <span class="act-body"><strong>${escHtml(e.agent)}</strong> &rarr; ${escHtml(e.step)}</span>
        ${timeTag}
      </div>`;
    case 'run':
      return `<div class="act-entry run ${e.status}">
        <span class="act-icon">&#129302;</span>
        <span class="act-body">${escHtml(e.text)}${e.detail ? ` <span class="act-detail">${escHtml(e.detail)}</span>` : ''}</span>
        ${timeTag}
      </div>`;
    default:
      return '';
  }
}

// Update the right panel when a new activity event arrives.
// (renderProgressView is invoked directly by ingestAgentEvent / progress / plan
// handlers; this wrapper is kept for any external callers.)
function renderActivityFeed() {
  renderProgressView();
}

// ── Right panel: files view ──
async function loadFileTree(taskId) {
  if (!taskId) { fileTree = []; renderFilesView(); return; }
  try {
    const resp = await fetch(`/api/tasks/${taskId}/files`);
    if (resp.ok) {
      fileTree = await resp.json();
      renderFilesView();
    }
  } catch(err) { /* ignore */ }
}

const FILE_ICONS = { md:'&#128221;', json:'&#128295;', txt:'&#128196;', csv:'&#128202;', tsv:'&#128202;', py:'&#128012;', rs:'&#9881;', js:'&#9881;', html:'&#127760;', css:'&#127912;', log:'&#128221;', ipynb:'&#129300;', png:'&#127912;', jpg:'&#127912;', jpeg:'&#127912;', svg:'&#127912;', gif:'&#127912;', webp:'&#127912;' };
function fileIcon(node) {
  if (node.is_dir) return node.name === '.workflow' ? '&#9881;' : '&#128193;';
  return FILE_ICONS[node.ext?.toLowerCase()] || '&#128196;';
}

function fmtSize(n) {
  if (n < 1024) return n + ' B';
  if (n < 1024*1024) return (n/1024).toFixed(1) + ' KB';
  return (n/1024/1024).toFixed(1) + ' MB';
}

function renderFilesView() {
  const el = document.getElementById('rpFiles');
  if (!el) return;
  if (!currentTaskId) {
    el.innerHTML = `<div class="rp-empty"><div class="icon">&#128193;</div><div>Select a task to browse its output files.</div></div>`;
    return;
  }
  if (!fileTree || fileTree.length === 0) {
    el.innerHTML = `<div class="rp-empty"><div class="icon">&#128193;</div><div>No files yet. They appear once the task produces output.</div></div>`;
    return;
  }
  const tree = renderFileNodes(fileTree);
  el.innerHTML = `<div class="file-toolbar">
      <span>Output files</span>
      <span class="count"></span>
      <button class="btn-refresh" onclick="viewProvenance()" title="Provenance 溯源">&#128279;</button>
      <button class="btn-refresh" onclick="loadFileTree(currentTaskId)" title="Refresh">&#8635;</button>
    </div>
    <div class="ftree">${tree}</div>`;
  // wire click handlers
  el.querySelectorAll('.ftree-dir-head').forEach(h => {
    h.addEventListener('click', () => h.parentElement.classList.toggle('collapsed'));
  });
  el.querySelectorAll('.ftree-file').forEach(f => {
    f.querySelector('.act-preview')?.addEventListener('click', () => openPreview(f.dataset.path));
    f.querySelector('.act-download')?.addEventListener('click', () => downloadFile(f.dataset.path));
    f.addEventListener('click', (e) => { if (!e.target.closest('.fact')) openPreview(f.dataset.path); });
  });
}

function renderFileNodes(nodes) {
  return nodes.map(n => {
    if (n.is_dir) {
      const kids = n.children && n.children.length ? `<div class="ftree-children">${renderFileNodes(n.children)}</div>` : '';
      return `<div class="ftree-dir">
        <div class="ftree-dir-head"><span class="chevron">&#9660;</span><span>${fileIcon(n)}</span><span>${escHtml(n.name)}</span></div>
        ${kids}</div>`;
    }
    return `<div class="ftree-file" data-path="${escHtml(n.path)}">
      <span class="ficon">${fileIcon(n)}</span>
      <span class="fname">${escHtml(n.name)}</span>
      <span class="fsize">${fmtSize(n.size)}</span>
      <button class="fact act-download" title="Download" onclick="event.stopPropagation()">&#11015;</button>
      <button class="fact act-preview" title="Preview" onclick="event.stopPropagation()">&#128065;</button>
    </div>`;
  }).join('');
}

function downloadFile(path) {
  if (!currentTaskId) return;
  const a = document.createElement('a');
  a.href = `/api/download/${currentTaskId}/${encodeURIComponent(path)}`;
  a.download = '';
  document.body.appendChild(a); a.click(); a.remove();
}

async function openPreview(path) {
  if (!currentTaskId) return;
  const overlay = document.getElementById('previewOverlay');
  const body = document.getElementById('previewBody');
  const title = document.getElementById('previewTitle');
  const meta = document.getElementById('previewMeta');
  const dl = document.getElementById('previewDownload');
  title.textContent = path.split('/').pop();
  meta.textContent = path;
  dl.onclick = () => downloadFile(path);
  body.className = 'preview-body';
  body.innerHTML = `<div class="preview-loading"><span class="cursor"></span> Loading…</div>`;
  overlay.classList.add('active');
  try {
    const resp = await fetch(`/api/tasks/${currentTaskId}/preview/${encodeURIComponent(path)}`);
    const data = await resp.json();
    if (!data.preview) {
      body.className = 'preview-body pv-raw';
      body.innerHTML = `Binary file (${fmtSize(data.size)}).<br>Use download to access it.`;
      return;
    }
    const ext = (data.ext || '').toLowerCase();
    if (['png','jpg','jpeg','gif','svg','webp'].includes(ext)) {
      // 图片（notebook 图表、管线绘图）经 raw 路由内联渲染。
      body.className = 'preview-body pv-image';
      body.innerHTML = `<img class="pv-img" src="/api/tasks/${currentTaskId}/raw/${encodeURIComponent(path)}" alt="${escHtml(path)}">`;
      return;
    }
    if (ext === 'ipynb') {
      body.className = 'preview-body rich nb-preview';
      const html = renderNotebook(data.content);
      if (html === null) {
        body.className = 'preview-body pv-text';
        body.textContent = prettyJson(data.content);
      } else {
        body.innerHTML = html;
        enhanceRichBody(body);
      }
      return;
    }
    if (ext === 'md') {
      body.className = 'preview-body rich md-preview';
      body.innerHTML = md(data.content);
      enhanceRichBody(body);
    } else if (ext === 'json') {
      body.className = 'preview-body pv-text';
      body.textContent = prettyJson(data.content);
    } else if (ext === 'csv' || ext === 'tsv') {
      body.className = 'preview-body rich';
      body.innerHTML = csvToTable(data.content, ext === 'tsv' ? '\t' : ',');
    } else {
      body.className = 'preview-body pv-text';
      body.textContent = data.content;
    }
    if (data.truncated) {
      const note = document.createElement('div');
      note.className = 'preview-truncated';
      note.textContent = `Preview truncated (${fmtSize(data.size)} total). Download for full content.`;
      body.appendChild(note);
    }
  } catch(err) {
    body.className = 'preview-body pv-raw';
    body.innerHTML = `Failed to load preview: ${escHtml(err.message)}`;
  }
}
function closePreview() { document.getElementById('previewOverlay').classList.remove('active'); }

function prettyJson(s) {
  try { return JSON.stringify(JSON.parse(s), null, 2); }
  catch { return s; }
}

function csvToTable(text, sep) {
  const lines = text.trim().split(/\r?\n/).filter(l => l.length);
  if (!lines.length) return escHtml(text);
  const rows = lines.map(l => l.split(sep));
  const head = rows[0].map(c => `<th>${escHtml(c)}</th>`).join('');
  const body = rows.slice(1, 200).map(r => `<tr>${r.map(c => `<td>${escHtml(c)}</td>`).join('')}</tr>`).join('');
  return `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

// ── Jupyter notebook preview（.ipynb 单元格级渲染）─────────────
// markdown cell 经 marked 渲染；code cell 显示源码与执行输出
// （stream 文本、HTML 结果、base64 图片、错误回溯）。
function nbSource(src) {
  return Array.isArray(src) ? src.join('') : String(src ?? '');
}

function renderNotebook(jsonText) {
  let nb;
  try { nb = JSON.parse(jsonText); } catch { return null; }
  if (!nb || !Array.isArray(nb.cells)) return null;
  const parts = [];
  for (const cell of nb.cells) {
    const type = cell.cell_type;
    const src = nbSource(cell.source);
    if (type === 'markdown') {
      parts.push(`<div class="nb-cell nb-md">${md(src)}</div>`);
      continue;
    }
    // code cell: source + outputs
    let outputsHtml = '';
    for (const o of (cell.outputs || [])) {
      const data = o.data || {};
      if (data && data['image/png']) {
        outputsHtml += `<div class="nb-out"><img src="data:image/png;base64,${data['image/png']}"></div>`;
      } else if (data && data['image/svg+xml']) {
        outputsHtml += `<div class="nb-out"><img src="data:image/svg+xml;utf8,${encodeURIComponent(data['image/svg+xml'])}"></div>`;
      } else if (data && data['text/html']) {
        outputsHtml += `<div class="nb-out nb-out-html">${nbSource(data['text/html'])}</div>`;
      } else if (data && data['text/plain']) {
        outputsHtml += `<div class="nb-out"><pre>${escHtml(nbSource(data['text/plain']))}</pre></div>`;
      } else if (o.output_type === 'stream') {
        outputsHtml += `<div class="nb-out nb-stdout"><pre>${escHtml(nbSource(o.text))}</pre></div>`;
      } else if (o.output_type === 'error') {
        outputsHtml += `<div class="nb-out nb-error"><pre>${escHtml((o.traceback || []).join('\n') || (o.ename + ': ' + o.evalue))}</pre></div>`;
      }
    }
    parts.push(`<div class="nb-cell nb-code">
      <div class="nb-code-src"><span class="nb-lang">In</span><pre>${escHtml(src)}</pre></div>
      ${outputsHtml ? `<div class="nb-code-out">${outputsHtml}</div>` : ''}
    </div>`);
  }
  const kernel = nb.metadata?.kernelspec?.display_name;
  return `<div class="nb-wrap">${parts.join('') || '<div class="nb-empty">(empty notebook)</div>'}
    ${kernel ? `<div class="nb-meta">kernel: ${escHtml(kernel)} · nbformat ${escHtml(String(nb.nbformat ?? ''))}</div>` : ''}</div>`;
}

// ── 结构化假说/辩论卡片 ────────────────────────────────────────
// research 管线完成后端会推送 type=hypotheses 事件：每条假说渲染为
// 结构化卡片（statement/mechanism/置信度条/裁决/支持与反驳要点/
// rebuttal），支持实时与历史重绘两个入口。
const VERDICT_META = {
  accept:    { label: 'Accept', cls: 'ok',    icon: '&#9989;' },
  revise:    { label: 'Revise', cls: 'warn',  icon: '&#128295;' },
  reject:    { label: 'Reject', cls: 'err',   icon: '&#10060;' },
  undebated: { label: 'Undebated', cls: 'idle', icon: '&#128196;' },
};

function bulletList(items, cls) {
  if (!Array.isArray(items) || items.length === 0) return '';
  return `<ul class="hyp-list ${cls || ''}">${items.map(x =>
    `<li>${escHtml(typeof x === 'string' ? x : JSON.stringify(x))}</li>`).join('')}</ul>`;
}

function showHypothesisCards(cards, container, cross) {
  const el = container || document.querySelector('#messages .messages-inner');
  if (!el) return;
  const wrap = document.createElement('div');
  wrap.className = 'msg msg-ai hyp-cards-msg';
  const cardsHtml = cards.map((h, i) => {
    const verdict = VERDICT_META[h.verdict?.toLowerCase()] || VERDICT_META.undebated;
    const conf = Math.round((h.confidence ?? 0) * 100);
    const confAfter = (typeof h.confidence_after === 'number')
      ? Math.round(h.confidence_after * 100) : null;
    return `<div class="hyp-card verdict-${verdict.cls}">
      <div class="hyp-head">
        <span class="hyp-idx">H${i + 1}</span>
        <span class="hyp-verdict ${verdict.cls}">${verdict.icon} ${verdict.label}</span>
        <span class="hyp-conf" title="debate confidence">
          <span class="hyp-conf-bar"><span style="width:${conf}%"></span></span>
          <span class="hyp-conf-num">${conf}%</span>
          ${confAfter !== null ? `<span class="hyp-conf-after">&rarr; ${confAfter}%</span>` : ''}
        </span>
      </div>
      <div class="hyp-statement">${escHtml(h.statement || '')}</div>
      ${h.mechanism ? `<details class="hyp-sec"><summary>Mechanism</summary><div>${md(h.mechanism)}</div></details>` : ''}
      ${bulletList(h.supporting_points, 'hyp-support') ? `<div class="hyp-sec-title">支持要点</div>${bulletList(h.supporting_points, 'hyp-support')}` : ''}
      ${bulletList(h.contradicting_points, 'hyp-contrast') ? `<div class="hyp-sec-title">反驳/矛盾</div>${bulletList(h.contradicting_points, 'hyp-contrast')}` : ''}
      ${bulletList(h.rebuttal_points, 'hyp-rebuttal') ? `<div class="hyp-sec-title">正方 Rebuttal</div>${bulletList(h.rebuttal_points, 'hyp-rebuttal')}` : ''}
      ${h.refinement_notes ? `<div class="hyp-notes"><span class="hyp-sec-title">Refinement</span>${escHtml(h.refinement_notes)}</div>` : ''}
    </div>`;
  }).join('');
  wrap.innerHTML = `<div class="msg-bubble rich"><div class="hyp-cards-title">&#128300; 致病机理假说（${cards.length} 条，含辩论裁决）</div><div class="hyp-cards">${cardsHtml}</div>${renderCrossComparison(cross, cards)}</div>`;
  el.appendChild(wrap);
  scrollBottom();
}

// ── 辩论交叉比较（judge 的跨假说裁决）──────────────────────────
// contradictions_between（矛盾对）+ ranking_rationale + 最强假说 +
// merge_suggestions：假说之间的"证据-矛盾之处的辩论和比较"结论区。
function hypShortRef(id, cards) {
  const idx = cards.findIndex(c => c.id === id);
  return idx >= 0 ? `H${idx + 1}` : (id || '').slice(0, 8);
}

function renderCrossComparison(cross, cards) {
  if (!cross) return '';
  const contradictions = Array.isArray(cross.contradictions) ? cross.contradictions : [];
  const merges = Array.isArray(cross.merge_suggestions) ? cross.merge_suggestions : [];
  const rationale = cross.ranking_rationale || '';
  if (!contradictions.length && !rationale && !cross.strongest && !merges.length) return '';
  const contraHtml = contradictions.map(c => `
    <div class="cross-pair">
      <span class="cross-vs">${escHtml(hypShortRef(c.a?.id, cards))} vs ${escHtml(hypShortRef(c.b?.id, cards))}</span>
      <span class="cross-reason">${escHtml(c.reason || '')}</span>
    </div>`).join('');
  return `<div class="cross-compare">
    <div class="cross-title">&#9878;&#65039; 交叉比较（跨假说裁决）</div>
    ${cross.strongest ? `<div class="cross-strongest">&#128170; 最强假说：${escHtml(hypShortRef(cross.strongest.id, cards))} — ${escHtml((cross.strongest.statement || '').slice(0, 160))}</div>` : ''}
    ${rationale ? `<div class="cross-rationale"><span class="hyp-sec-title">排序依据</span>${escHtml(rationale)}</div>` : ''}
    ${contraHtml ? `<div class="cross-pairs"><span class="hyp-sec-title">假说间矛盾（${contradictions.length} 对）</span>${contraHtml}</div>` : ''}
    ${merges.length ? `<div class="cross-merges"><span class="hyp-sec-title">合并/精简建议</span>${bulletList(merges, 'hyp-support')}</div>` : ''}
  </div>`;
}

// ── 验证计划卡片（目标3：数据分析任务 + 湿实验方案）─────────────
// validation 阶段完成后端推送 type=validation 事件：每个假说一张计划卡，
// DA 任务行显示数据集徽章（GEO accession 等）、统计方法与预期结果；
// 湿实验协议折叠展示步骤/对照/时间线。
function datasetBadge(src) {
  if (!src || typeof src !== 'object') return '';
  const kind = String(src.kind || '').toUpperCase();
  const val = src.value || '';
  const label = kind === 'GEO' && val ? `GEO · ${val}`
    : kind === 'LOCAL' ? `本地 · ${val}`
    : kind === 'CUSTOM_URL' ? 'URL'
    : kind || '';
  return `<span class="ds-badge ds-${escHtml(String(src.kind || 'other'))}">${escHtml(label)}</span>`;
}

function showValidationCards(plans, container) {
  const el = container || document.querySelector('#messages .messages-inner');
  if (!el) return;
  const wrap = document.createElement('div');
  wrap.className = 'msg msg-ai val-cards-msg';
  const totalDa = plans.reduce((n, p) => n + (p.data_analysis_tasks?.length || 0), 0);
  const totalWl = plans.reduce((n, p) => n + (p.wet_lab_protocols?.length || 0), 0);
  const cardsHtml = plans.map((p) => {
    const das = Array.isArray(p.data_analysis_tasks) ? p.data_analysis_tasks : [];
    const wls = Array.isArray(p.wet_lab_protocols) ? p.wet_lab_protocols : [];
    const daHtml = das.map(t => `
      <div class="da-task">
        <div class="da-head">
          <span class="da-id">${escHtml(t.id || '')}</span>
          ${datasetBadge(t.dataset_source)}
          <span class="da-method">${escHtml(t.statistical_method || '')}</span>
          <span class="da-prio" title="priority">${Math.round((t.priority ?? 0.5) * 100)}%</span>
        </div>
        <div class="da-obj">${escHtml(t.objective || '')}</div>
        <div class="da-meta">
          ${t.cohort_definition ? `<span><b>队列：</b>${escHtml(t.cohort_definition)}</span>` : ''}
          ${t.expected_outcome ? `<span><b>预期：</b>${escHtml(t.expected_outcome)}</span>` : ''}
          ${t.deliverable ? `<span><b>交付：</b>${escHtml(t.deliverable)}</span>` : ''}
        </div>
      </div>`).join('');
    const wlHtml = wls.map(w => `
      <details class="wl-proto">
        <summary>
          <span class="da-id">${escHtml(w.id || '')}</span>
          <span class="wl-obj">${escHtml(w.objective || '')}</span>
          ${w.timeline_days ? `<span class="wl-days">${escHtml(String(w.timeline_days))} 天</span>` : ''}
          <span class="da-prio" title="feasibility">${Math.round((w.feasibility ?? 0.5) * 100)}%</span>
        </summary>
        <div class="wl-body">
          ${bulletList(w.steps, 'wl-steps') ? `<div class="hyp-sec-title">步骤</div>${bulletList(w.steps, 'wl-steps')}` : ''}
          ${bulletList(w.controls, 'hyp-support') ? `<div class="hyp-sec-title">对照</div>${bulletList(w.controls, 'hyp-support')}` : ''}
          ${w.expected_outcome ? `<div class="da-meta"><span><b>预期：</b>${escHtml(w.expected_outcome)}</span></div>` : ''}
          ${bulletList(w.reagents, 'wl-reagents') ? `<div class="hyp-sec-title">试剂</div>${bulletList(w.reagents, 'wl-reagents')}` : ''}
        </div>
      </details>`).join('');
    return `<div class="val-card">
      <div class="val-head">
        <span class="val-idx">Plan ${escHtml(String((p.index ?? 0) + 1))}</span>
        <span class="val-count">${das.length} 数据分析 · ${wls.length} 湿实验</span>
      </div>
      ${p.statement ? `<div class="val-statement">${escHtml(p.statement)}</div>` : ''}
      ${p.rationale ? `<div class="val-rationale">${escHtml(p.rationale)}</div>` : ''}
      ${daHtml ? `<div class="hyp-sec-title">数据分析任务（可端到端执行）</div><div class="da-list">${daHtml}</div>` : ''}
      ${wlHtml ? `<div class="hyp-sec-title">湿实验方案</div><div class="wl-list">${wlHtml}</div>` : ''}
    </div>`;
  }).join('');
  wrap.innerHTML = `<div class="msg-bubble rich"><div class="hyp-cards-title">&#129514; 验证任务计划（${plans.length} 个假说 · ${totalDa} 个数据分析任务 · ${totalWl} 个湿实验方案）</div><div class="hyp-cards">${cardsHtml}</div></div>`;
  el.appendChild(wrap);
  scrollBottom();
}

// ── Provenance（溯源审计）面板 ────────────────────────────────
// 后端 /api/provenance/{task_id} 汇总 analysis/**/provenance.json：
// 脚本/输入输出哈希、conda 环境、seed、git commit、repair 历史。
async function viewProvenance() {
  if (!currentTaskId) return;
  const overlay = document.getElementById('previewOverlay');
  const body = document.getElementById('previewBody');
  const title = document.getElementById('previewTitle');
  const meta = document.getElementById('previewMeta');
  const dl = document.getElementById('previewDownload');
  title.textContent = 'Provenance 溯源';
  meta.textContent = currentTaskId;
  dl.onclick = null;
  body.className = 'preview-body rich';
  body.innerHTML = `<div class="preview-loading"><span class="cursor"></span> Loading provenance…</div>`;
  overlay.classList.add('active');
  try {
    const resp = await fetch(`/api/provenance/${currentTaskId}`);
    const data = await resp.json();
    if (data.error) {
      body.innerHTML = `<div class="pv-empty">${escHtml(data.error)}</div>`;
      return;
    }
    const records = Array.isArray(data.records) ? data.records : [];
    if (!records.length) {
      body.innerHTML = `<div class="pv-empty">No provenance records found.</div>`;
      return;
    }
    body.innerHTML = records.map((r, i) => {
      const rec = r.provenance || r.record || {};
      const kv = (k) => rec[k] !== undefined && rec[k] !== null && `${rec[k]}`.length
        ? `<div class="pv-kv"><span class="pv-k">${escHtml(k)}</span><span class="pv-v"><code>${escHtml(typeof rec[k] === 'string' ? rec[k] : JSON.stringify(rec[k]))}</code></span></div>` : '';
      const repairs = Array.isArray(rec.repair_history) && rec.repair_history.length
        ? `<div class="pv-kv"><span class="pv-k">repair_history</span><span class="pv-v">${rec.repair_history.map(x => `<div>&#8594; attempt ${escHtml(String(x.attempt ?? ''))}: ${escHtml(x.action || '')} ${x.error_tail ? `<code>${escHtml(String(x.error_tail).slice(0, 200))}</code>` : ''}</div>`).join('')}</span></div>` : '';
      return `<details class="pv-rec" ${i === 0 ? 'open' : ''}>
        <summary>${escHtml(rec.task_id || r.path || `record ${i + 1}`)}</summary>
        <div class="pv-path">${escHtml(r.path || '')}</div>
        ${kv('hypothesis_ref')}${kv('script_path')}${kv('script_hash')}${kv('seed')}${kv('conda_env')}${kv('conda_used')}${kv('git_commit')}${kv('exit_code')}${kv('execution_backend')}${kv('notebook_path')}${kv('notebook_executed')}${kv('duration')}
        ${kv('inputs')}${kv('outputs')}${kv('package_versions')}${repairs}
      </details>`;
    }).join('');
  } catch (err) {
    body.innerHTML = `<div class="pv-empty">Failed to load provenance: ${escHtml(err.message)}</div>`;
  }
}

// ── Helpers ──
function scrollBottom() { const el = document.getElementById('messages'); el.scrollTop = el.scrollHeight; }
function escHtml(s) { const d = document.createElement('div'); d.textContent = (s == null ? '' : s); return d.innerHTML; }
const esc = escHtml;
function showToast(msg, isError, cls) {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = 'toast show' + (isError ? ' error' : cls ? ' '+cls : '');
  setTimeout(() => el.className = 'toast', 3000);
}

// ── Markdown (use marked.js parser) ──
function md(text) {
  if (!text) return '';
  if (typeof marked !== 'undefined' && marked.parse) {
    return marked.parse(text, { breaks: true, gfm: true });
  }
  // Fallback if marked is not loaded
  return escHtml(text).replace(/\n/g, '<br>');
}

// Post-process rendered markdown: external links open in a new tab safely.
function enhanceRichBody(root) {
  if (!root) return;
  root.querySelectorAll('a[href^="http"]').forEach(a => {
    a.target = '_blank';
    a.rel = 'noopener noreferrer';
  });
}

// ── Init ──
renderProgressView();
renderFilesView();
connect();

// ── Model registry（运行时 LLM 管理：统一设置中心） ───────────
//
// Single source of truth in the frontend: `settingsStore`. Re-hydrated from
// `/api/settings/active` (one round-trip) and `/api/kinds`. The header
// model chip and the settings drawer both render from this store, so the
// two surfaces never drift.
//
// Legacy references (openModelModal / renderModelSelect / etc.) have been
// replaced by `openSettings(tab)` + `renderModelChip`.

let settingsStore = {
  activeId: null,
  active: null,        // ModelProfileView
  models: [],          // ModelProfileView[]
  kinds: [],           // KindView[]  (slug/label/icon/default_base_url)
  debate: { proposer: null, opponent: null, judge: null },
  settingsTab: 'models',
};

async function loadSettings() {
  try {
    const [active, models, kinds, debate] = await Promise.all([
      fetch('/api/settings/active').then(r => r.ok ? r.json() : null),
      fetch('/api/models').then(r => r.ok ? r.json() : null),
      fetch('/api/kinds').then(r => r.ok ? r.json() : null),
      fetch('/api/debate-models').then(r => r.ok ? r.json() : null),
    ]);
    settingsStore.active = active?.active ?? null;
    settingsStore.activeId = active?.active?.id ?? models?.active_id ?? null;
    settingsStore.models = models?.models ?? [];
    settingsStore.kinds = (active?.kinds) ?? (kinds?.kinds) ?? [];
    settingsStore.debate = {
      proposer: debate?.proposer ?? active?.debate?.proposer ?? null,
      opponent: debate?.opponent ?? active?.debate?.opponent ?? null,
      judge:    debate?.judge    ?? active?.debate?.judge    ?? null,
    };
    renderModelChip();
    if (document.getElementById('settingsOverlay')?.classList.contains('active')) {
      renderSettingsTab();
    }
  } catch (e) { /* offline — chip stays at placeholder */ }
}

function renderModelChip() {
  const icon = document.getElementById('modelChipIcon');
  const name = document.getElementById('modelChipName');
  const summary = document.getElementById('modelSummary');
  const m = settingsStore.active || settingsStore.models.find(x => x.id === settingsStore.activeId);
  if (!m) {
    if (icon) icon.textContent = '🤖';
    if (name) name.textContent = '未选择模型';
    if (summary) summary.textContent = '';
    return;
  }
  if (icon) icon.textContent = m.kind_icon || '🤖';
  if (name) name.textContent = m.display_name;
  if (summary) summary.textContent = `${m.flash_model_name || m.model_name}${m.pro_model_name_effective && m.pro_model_name_effective !== m.flash_model_name ? ' · Pro ' + m.pro_model_name_effective : ''}`;
}

async function activateModel(id) {
  if (!id || id === settingsStore.activeId) return;
  try {
    const res = await fetch(`/api/models/${encodeURIComponent(id)}/activate`, { method: 'POST' });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) { showToast(data.error || '切换失败', true); return; }
    showToast(`已切换到 ${settingsStore.models.find(m => m.id === id)?.display_name || id}`, false, 'success');
    await loadSettings();
    if (document.getElementById('settingsOverlay')?.classList.contains('active')) renderSettingsTab();
  } catch (e) { showToast('切换失败: ' + e.message, true); }
}

function openSettings(tab) {
  const overlay = document.getElementById('settingsOverlay');
  if (!overlay) return;
  if (tab && ['models','debate','about'].includes(tab)) settingsStore.settingsTab = tab;
  overlay.classList.add('active');
  for (const t of document.querySelectorAll('.settings-tab')) {
    t.classList.toggle('active', t.dataset.tab === settingsStore.settingsTab);
  }
  renderSettingsTab();
  // Hydrate fresh data so changes from other surfaces appear.
  loadSettings();
}
function closeSettings() {
  document.getElementById('settingsOverlay')?.classList.remove('active');
}
function switchSettingsTab(tab) {
  settingsStore.settingsTab = tab;
  for (const t of document.querySelectorAll('.settings-tab')) {
    t.classList.toggle('active', t.dataset.tab === tab);
  }
  renderSettingsTab();
}

function renderSettingsTab() {
  const body = document.getElementById('settingsBody');
  if (!body) return;
  if (settingsStore.settingsTab === 'models') renderSettingsModels(body);
  else if (settingsStore.settingsTab === 'debate') renderSettingsDebate(body);
  else renderSettingsAbout(body);
}

// ── Settings · Models tab ───────────────────────────────────
function renderSettingsModels(body) {
  const builtin = settingsStore.models.filter(m => m.builtin);
  const custom = settingsStore.models.filter(m => !m.builtin);
  const activeModel = settingsStore.active || settingsStore.models.find(m => m.id === settingsStore.activeId);
  const kindsHtml = settingsStore.kinds.map(k =>
    `<option value="${esc(k.slug)}" data-default-base="${esc(k.default_base_url)}">${esc(k.icon)} ${esc(k.label)}</option>`
  ).join('');

  body.innerHTML = `
    <div class="settings-section">
      <h4>当前模型 <span class="hsp-hint">点击其它卡片即可切换</span></h4>
      ${activeModel ? renderModelCard(activeModel, true) : emptyModels()}
    </div>

    <div class="settings-section">
      <h4>内置模型 <span class="hsp-hint">${builtin.length}</span></h4>
      ${builtin.length ? builtin.map(m => renderModelCard(m, m.id === settingsStore.activeId)).join('') : emptyModels()}
    </div>

    <div class="settings-section">
      <h4>自定义模型 <span class="hsp-hint">${custom.length}</span></h4>
      ${custom.length ? custom.map(m => renderModelCard(m, m.id === settingsStore.activeId)).join('') :
        '<div class="empty-state"><div class="es-icon">🪄</div>未添加自定义模型。<br>支持任意 OpenAI / Anthropic 兼容端点。</div>'}
    </div>

    <hr class="divider">

    <div class="settings-section">
      <h4>添加自定义模型</h4>
      <div class="form-grid">
        <div class="form-row span2">
          <label>显示名称</label>
          <input id="mfName" type="text" placeholder="例如：SiliconFlow GLM-4.7">
        </div>
        <div class="form-row">
          <label>类型</label>
          <select id="mfKind">${kindsHtml}</select>
        </div>
        <div class="form-row">
          <label>Base URL <small id="mfKindHint"></small></label>
          <input id="mfBaseUrl" type="text" placeholder="留空使用类型默认值">
        </div>
        <div class="form-row">
          <label>Flash 模型名</label>
          <input id="mfModel" type="text" placeholder="例如：glm-4.7">
        </div>
        <div class="form-row">
          <label>Pro 模型名 <small>（可选）</small></label>
          <input id="mfProModel" type="text" placeholder="留空则与 Flash 相同">
        </div>
        <div class="form-row span2">
          <label>API Key</label>
          <input id="mfKey" type="password" placeholder="key 写入服务端 models.json（已 gitignore）">
        </div>
      </div>
      <p class="form-hint">切换类型会清空 Base URL 输入框，可填入或留空使用默认值。</p>
      <div class="btn-row">
        <button class="btn-action" onclick="resetModelForm()">重置</button>
        <button class="btn-action primary" onclick="addModel()">添加模型</button>
      </div>
    </div>
  `;
  wireModelForm();
}

function emptyModels() {
  return '<div class="empty-state"><div class="es-icon">🤖</div>暂无可用模型</div>';
}

function renderModelCard(m, isActive) {
  const tier = (m.flash_model_name && m.pro_model_name_effective && m.flash_model_name !== m.pro_model_name_effective)
    ? `<div class="cm-row"><span class="cm-key">Flash</span><span>${esc(m.flash_model_name)}</span></div>
       <div class="cm-row"><span class="cm-key">Pro</span><span>${esc(m.pro_model_name_effective)}</span></div>`
    : `<div class="cm-row"><span class="cm-key">Model</span><span>${esc(m.flash_model_name || m.model_name)}</span></div>`;
  return `
    <div class="card ${isActive ? 'active' : ''}">
      <div class="card-head">
        <div class="card-icon">${esc(m.kind_icon || '🤖')}</div>
        <div class="grow">
          <div class="card-title">${esc(m.display_name)}
            ${isActive ? '<span class="status-tag ok">使用中</span>' : ''}
            ${m.builtin ? '<span class="status-tag muted">内置</span>' : '<span class="status-tag muted">自定义</span>'}
            ${!m.has_key ? '<span class="status-tag warn">无 Key</span>' : ''}
          </div>
          <div class="card-sub">${esc(m.kind_label)} · ${esc(m.base_url || '(无 base url)')}</div>
        </div>
        <div class="card-actions">
          ${!isActive ? `<button class="btn-action primary" onclick="activateModel('${esc(m.id)}')">使用</button>` : ''}
          ${!m.builtin ? `<button class="btn-action danger" onclick="deleteModel('${esc(m.id)}','${esc(m.display_name)}')">删除</button>` : ''}
        </div>
      </div>
      <div class="card-meta">${tier}<div class="cm-row"><span class="cm-key">Key</span><span>${esc(m.api_key_masked || '—')}</span></div></div>
    </div>
  `;
}

function wireModelForm() {
  const sel = document.getElementById('mfKind');
  const hint = document.getElementById('mfKindHint');
  const url = document.getElementById('mfBaseUrl');
  if (!sel) return;
  const syncHint = () => {
    const opt = sel.selectedOptions[0];
    const def = opt?.dataset?.defaultBase || '';
    if (hint) hint.textContent = def ? `默认 ${def}` : '需手动填写';
    if (url && !url.value && def) url.placeholder = `默认 ${def}`;
  };
  sel.onchange = syncHint;
  syncHint();
}

async function addModel() {
  const name = document.getElementById('mfName')?.value.trim();
  const kind = document.getElementById('mfKind')?.value;
  const baseUrl = document.getElementById('mfBaseUrl')?.value.trim();
  const modelName = document.getElementById('mfModel')?.value.trim();
  const proModel = document.getElementById('mfProModel')?.value.trim();
  const apiKey = document.getElementById('mfKey')?.value.trim();
  if (!name) { showToast('请输入显示名称', true); return; }
  if (!modelName) { showToast('请输入模型名', true); return; }
  if (!apiKey) { showToast('请输入 API Key', true); return; }
  try {
    const res = await fetch('/api/models', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        display_name: name, kind, base_url: baseUrl, model_name: modelName,
        pro_model_name: proModel || null, api_key: apiKey,
      }),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) { showToast(data.error || '添加失败', true); return; }
    showToast(`已添加 ${name}`, false, 'success');
    resetModelForm();
    await loadSettings();
    renderSettingsTab();
  } catch (e) { showToast('添加失败: ' + e.message, true); }
}

function resetModelForm() {
  for (const id of ['mfName','mfBaseUrl','mfModel','mfProModel','mfKey']) {
    const el = document.getElementById(id);
    if (el) el.value = '';
  }
}

async function deleteModel(id, displayName) {
  if (!confirm(`删除模型配置 "${displayName}"？`)) return;
  try {
    const res = await fetch(`/api/models/${encodeURIComponent(id)}`, { method: 'DELETE' });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) { showToast(data.error || '删除失败', true); return; }
    showToast(`已删除 ${displayName}`, false, 'success');
    await loadSettings();
    renderSettingsTab();
  } catch (e) { showToast('删除失败: ' + e.message, true); }
}

// ── Settings · Debate tab ───────────────────────────────────
function renderSettingsDebate(body) {
  body.innerHTML = `
    <div class="settings-section">
      <h4>辩论 / 角色模型</h4>
      <p class="form-hint" style="margin-bottom:14px">辩论模式下，每个角色（正方 / 反方 / 裁判）可使用不同模型；默认全部沿用主模型。设置对 Web 辩论模式与 CLI（<code>debate</code> / <code>research</code> 管线）同时生效。</p>
      ${renderRoleRow('proposer', '正方 Proposer', '提出假说或论点', '🟢', debateRoleOptions(settingsStore.debate.proposer))}
      ${renderRoleRow('opponent', '反方 Opponent', '反驳、寻找反例', '🔴', debateRoleOptions(settingsStore.debate.opponent))}
      ${renderRoleRow('judge', '裁判 Judge', '综合证据做出裁决', '⚖', debateRoleOptions(settingsStore.debate.judge))}
      <div class="btn-row" style="margin-top:14px">
        <button class="btn-action" onclick="resetDebateRoles()">重置为主模型</button>
        <button class="btn-action primary" onclick="saveDebateRoles()">保存</button>
      </div>
    </div>
  `;
}

function debateRoleOptions(selected) {
  const def = `<option value="" ${selected ? '' : 'selected'}>主模型（默认）</option>`;
  const opts = settingsStore.models.map(m =>
    `<option value="${esc(m.id)}" ${m.id === selected ? 'selected' : ''}>${esc(m.kind_icon || '')} ${esc(m.display_name)} · ${esc(m.flash_model_name || m.model_name)}</option>`
  ).join('');
  return def + opts;
}

function renderRoleRow(role, title, hint, icon, options) {
  return `
    <div class="role-card">
      <div class="role-icon">${icon}</div>
      <div class="role-label">
        <div class="rl-title">${title}</div>
        <div class="rl-hint">${hint}</div>
      </div>
      <select id="dr${title.split(' ')[0][0].toUpperCase() + title.split(' ')[0].slice(1)}">${options}</select>
    </div>
  `;
}

async function saveDebateRoles() {
  const roleIds = { proposer: 'drProposer', opponent: 'drOpponent', judge: 'drJudge' };
  const body = {};
  for (const r of Object.keys(roleIds)) {
    const v = document.getElementById(roleIds[r])?.value || '';
    body[r] = v.trim() ? v : null;
  }
  try {
    const res = await fetch('/api/debate-models', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) { showToast(data.error || '保存失败', true); return; }
    showToast('辩论角色设置已保存', false, 'success');
    await loadSettings();
  } catch (e) { showToast('保存失败: ' + e.message, true); }
}

async function resetDebateRoles() {
  try {
    const res = await fetch('/api/debate-models', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ proposer: null, opponent: null, judge: null }),
    });
    if (!res.ok) { showToast('重置失败', true); return; }
    showToast('已重置为主模型', false, 'success');
    await loadSettings();
    renderSettingsTab();
  } catch (e) { showToast('重置失败: ' + e.message, true); }
}

// ── Settings · About tab ────────────────────────────────────
function renderSettingsAbout(body) {
  const active = settingsStore.active;
  body.innerHTML = `
    <div class="settings-section">
      <h4>当前运行环境</h4>
      <div class="card">
        <div class="card-head">
          <div class="card-icon">🤖</div>
          <div class="grow">
            <div class="card-title">${active ? esc(active.display_name) : '未选择'}</div>
            <div class="card-sub">${active ? esc(active.kind_label) + ' · ' + esc(active.base_url) : '—'}</div>
          </div>
          <div class="card-actions">
            ${active?.has_key ? '<span class="status-tag ok">Active Key</span>' : '<span class="status-tag warn">No Key</span>'}
          </div>
        </div>
        <div class="card-meta">
          <div class="cm-row"><span class="cm-key">Flash</span><span>${active ? esc(active.flash_model_name) : '—'}</span></div>
          <div class="cm-row"><span class="cm-key">Pro</span><span>${active ? esc(active.pro_model_name_effective) : '—'}</span></div>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <h4>已配置的 LLM</h4>
      ${settingsStore.models.length ? settingsStore.models.map(m => `
        <div class="card" style="padding:8px 12px">
          <div style="display:flex;align-items:center;gap:8px">
            <span style="font-size:16px">${esc(m.kind_icon || '🤖')}</span>
            <span class="grow" style="font-weight:600">${esc(m.display_name)}</span>
            <span class="status-tag ${m.has_key ? 'ok' : 'warn'}">${m.has_key ? 'Key OK' : 'No Key'}</span>
            ${m.builtin ? '<span class="status-tag muted">内置</span>' : '<span class="status-tag muted">自定义</span>'}
          </div>
        </div>
      `).join('') : emptyModels()}
    </div>
    <div class="settings-section">
      <h4>关于</h4>
      <div class="card">
        <div class="card-head">
          <div class="card-icon">🧪</div>
          <div class="grow">
            <div class="card-title">Miniagent · Round 36</div>
            <div class="card-sub">统一设置中心 + 主题重构</div>
          </div>
        </div>
        <p class="form-hint">所有 LLM 配置、辩论角色、模型图标都来自后端（<code>/api/kinds</code>、<code>/api/models</code>、<code>/api/debate-models</code>、<code>/api/settings/active</code>），前端不再硬编码枚举。</p>
      </div>
    </div>
  `;
}
