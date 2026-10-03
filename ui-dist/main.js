// Лама Чат — логика интерфейса (полностью на русском)
// Мульти-чаты: у каждого своя модель и история; при переключении чата
// модель предыдущего выгружается, выбранная — загружается.

function initTauri() {
  const ok = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.event;
  if (!ok) return null;
  return {
    invoke: window.__TAURI__.core.invoke,
    listen: window.__TAURI__.event.listen,
  };
}
const tauri = initTauri();
if (!tauri) {
  document.body.innerHTML =
    '<div style="display:flex;height:100vh;align-items:center;justify-content:center;flex-direction:column;gap:12px;font-family:-apple-system,sans-serif;background:#0f1115;color:#ff8f8f;padding:30px;text-align:center">' +
    '<b>Интерфейс не смог связаться с приложением.</b>' +
    '<span style="color:#8b93a7;font-size:13px">Похоже, приложение запущено из старой сборки. Переустановите «Лама Чат» из папки Приложения.</span></div>';
  throw new Error('Tauri API недоступен');
}
const { invoke, listen } = tauri;
const showError = (e) => { console.error(e); showToast('Сбой интерфейса: ' + (e && e.message ? e.message : e)); };
window.addEventListener('error', (ev) => showError(ev.error || ev.message));
window.addEventListener('unhandledrejection', (ev) => showError(ev.reason));

// ── Состояние ──────────────────────────────────────────────
let busy = false;          // идёт генерация
let webSearch = false;    // включён ли веб-поиск
let lastCaretEl = null;    // мигающий курсор
let attachments = [];     // [{name, content}]
let chats = [];           // список чатов (мета)
let activeChat = null;    // мета активного чата
let messages = [];        // история активного чата

const $ = (id) => document.getElementById(id);
const chatEl = $('chat'), welcome = $('welcome'), input = $('input');
const btnSend = $('btn-send'), btnStop = $('btn-stop'), btnWeb = $('btn-web');
const btnContinue = $('btn-continue');
const dot = $('dot'), statusText = $('status-text');
const chatList = $('chat-list');

// ── Рендер сообщений ────────────────────────────────────────
function hideWelcome() { if (welcome) welcome.style.display = 'none'; }
function showWelcome() { if (welcome) welcome.style.display = 'flex'; }

function addMessage(role, text) {
  hideWelcome();
  const wrap = document.createElement('div');
  wrap.className = 'msg ' + (role === 'user' ? 'user' : 'ai');

  const avatar = document.createElement('div');
  avatar.className = 'avatar ' + (role === 'user' ? 'user' : 'ai');
  avatar.textContent = role === 'user' ? 'Я' : 'Л';

  const bubbleWrap = document.createElement('div');
  bubbleWrap.className = 'bubble-wrap';
  const who = document.createElement('div');
  who.className = 'who';
  who.textContent = role === 'user' ? 'Вы' : 'Лама';
  const bubble = document.createElement('div');
  bubble.className = 'bubble';
  bubble.textContent = text;
  bubbleWrap.appendChild(who);
  bubbleWrap.appendChild(bubble);
  wrap.appendChild(avatar);
  wrap.appendChild(bubbleWrap);
  chatEl.appendChild(wrap);
  chatEl.scrollTop = chatEl.scrollHeight;
  return bubble;
}

/// Сворачиваемый блок «размышлений» reasoning-модели.
function addThinkingBlock() {
  hideWelcome();
  const wrap = document.createElement('div');
  wrap.className = 'msg ai';

  const avatar = document.createElement('div');
  avatar.className = 'avatar ai';
  avatar.textContent = 'Л';

  const bubbleWrap = document.createElement('div');
  bubbleWrap.className = 'bubble-wrap';
  const who = document.createElement('div');
  who.className = 'who';
  who.textContent = 'Лама думает';

  const think = document.createElement('details');
  think.className = 'think';
  const summary = document.createElement('summary');
  summary.textContent = 'Показать размышления';
  const body = document.createElement('div');
  body.className = 'think-body';
  think.appendChild(summary);
  think.appendChild(body);

  bubbleWrap.appendChild(who);
  bubbleWrap.appendChild(think);
  wrap.appendChild(avatar);
  wrap.appendChild(bubbleWrap);
  chatEl.appendChild(wrap);
  chatEl.scrollTop = chatEl.scrollHeight;
  return body;
}

function addCaret(bubble) {
  const c = document.createElement('span');
  c.className = 'cursor';
  bubble.appendChild(c);
  lastCaretEl = c;
}
function removeCaret() {
  if (lastCaretEl) { lastCaretEl.remove(); lastCaretEl = null; }
}

function addSources(sources) {
  if (!sources || sources.length === 0) return;
  const lastAi = [...chatEl.querySelectorAll('.msg.ai .bubble-wrap')].pop();
  if (!lastAi) return;
  const box = document.createElement('div');
  box.className = 'sources';
  for (const s of sources) {
    const chip = document.createElement('div');
    chip.className = 'src';
    chip.textContent = '🔗 ' + s.title;
    chip.title = s.url;
    chip.onclick = () => invoke('open_url', { url: s.url }).catch(console.error);
    box.appendChild(chip);
  }
  lastAi.appendChild(box);
  chatEl.scrollTop = chatEl.scrollHeight;
}

// ── Управление чатами ───────────────────────────────────────
function renderChatList() {
  chatList.innerHTML = '';
  for (const c of chats) {
    const item = document.createElement('div');
    item.className = 'chat-item' + (activeChat && c.id === activeChat.id ? ' active' : '');
    const name = document.createElement('div');
    name.className = 'chat-name';
    name.textContent = c.title || 'Без названия';
    name.title = c.title + ' · ' + c.model;
    const del = document.createElement('div');
    del.className = 'chat-del';
    del.textContent = '✕';
    del.title = 'Удалить чат';
    del.onclick = async (ev) => {
      ev.stopPropagation();
      if (!confirm('Удалить чат «' + (c.title || 'Без названия') + '»?')) return;
      await invoke('delete_chat', { id: c.id }).catch(showError);
      await loadChats();
      if (activeChat && activeChat.id === c.id) {
        await switchToChat(chats[0] ? chats[0].id : null);
      }
    };
    item.onclick = () => switchToChat(c.id);
    item.appendChild(name);
    item.appendChild(del);
    chatList.appendChild(item);
  }
}

function renderSideModel() {
  const sel = $('model-picker');
  if (!sel) return;
  if (!activeChat) {
    sel.innerHTML = '<option value="">—</option>';
    return;
  }
  const cur = activeChat.model;
  // Заполняем только если состав изменился (сохраняем выделение).
  const key = knownModels.map((m) => m.id).join('|') + '#' + cur;
  if (sel.dataset.key !== key) {
    sel.dataset.key = key;
    sel.innerHTML = '';
    for (const m of knownModels) {
      const opt = document.createElement('option');
      opt.value = m.id;
      opt.textContent = m.name + (m.source ? '  ·  ' + m.source : '');
      if (m.id === cur) opt.selected = true;
      sel.appendChild(opt);
    }
    if (!knownModels.some((m) => m.id === cur)) {
      const opt = document.createElement('option');
      opt.value = cur;
      opt.textContent = cur.split('/').pop().split(':')[0] + '  ·  текущая';
      opt.selected = true;
      sel.appendChild(opt);
    }
  } else {
    sel.value = cur;
  }
}

let knownModels = [];

async function refreshModels() {
  knownModels = await invoke('list_models').catch(() => []);
  renderSideModel();
}

// Смена модели чата через селект в углу — сердце мульти-чата.
$('model-picker').onchange = async (e) => {
  const model = e.target.value;
  if (!activeChat || !model || model === activeChat.model) return;
  if (busy) { showToast('Дождитесь окончания ответа'); renderSideModel(); return; }
  activeChat.model = model;
  await invoke('update_chat', { meta: activeChat }).catch(showError);
  // Старая модель выгрузится, новая загрузится (ensure_model_loaded).
  await invoke('ensure_model_loaded', { model }).catch(showError);
};

async function loadChats() {
  chats = await invoke('list_chats').catch(() => []);
  // Если чатов нет — создаём первый с моделью из настроек.
  if (chats.length === 0) {
    const s = await invoke('get_settings').catch(() => ({ model: '' }));
    const meta = await invoke('create_chat', { model: s.model || null }).catch(() => null);
    if (meta) chats = [meta];
  }
}

/// Переключиться на чат: подгрузить историю и модель.
async function switchToChat(id) {
  if (busy) { showToast('Дождитесь окончания ответа'); return; }
  if (!id) {
    activeChat = null;
    messages = [];
    chatEl.innerHTML = '';
    showWelcome();
    renderChatList();
    renderSideModel();
    return;
  }
  const meta = chats.find((c) => c.id === id);
  if (!meta) return;
  activeChat = meta;

  // История чата.
  messages = await invoke('get_chat_history', { id }).catch(() => []);

  // Отрисовка.
  chatEl.innerHTML = '';
  if (messages.length === 0) {
    showWelcome();
  } else {
    hideWelcome();
    for (const m of messages) {
      if (m.role === 'assistant') addMessage('assistant', m.content);
      else addMessage(m.role === 'user' ? 'user' : 'assistant', m.content);
    }
  }
  renderChatList();
  renderSideModel();

  // Модель чата: если не та, что на сервере — перезагрузка.
  await invoke('ensure_model_loaded', { model: meta.model }).catch(showError);
}

/// Сохранить текущую историю в активный чат.
async function persistHistory() {
  if (activeChat) {
    await invoke('save_chat_history', { id: activeChat.id, messages }).catch(console.error);
  }
}

$('btn-new-chat').onclick = async () => {
  if (busy) { showToast('Дождитесь окончания ответа'); return; }
  // Модель — как у текущего чата (можно поменять в настройках).
  const model = activeChat ? activeChat.model : null;
  const meta = await invoke('create_chat', { model, title: null }).catch(showError);
  if (!meta) return;
  await loadChats();
  await switchToChat(meta.id);
  input.focus();
};

// ── Отправка сообщения ──────────────────────────────────────
async function send() {
  const text = input.value.trim();
  if (!text || busy) return;
  if (!activeChat) {
    // Чата нет (всё удалено) — создадим на лету.
    await $('btn-new-chat').onclick();
    if (!activeChat) return;
  }
  busy = true;
  setBusyUI(true);

  messages.push({ role: 'user', content: text });
  addMessage('user', text + (attachments.length ? `\n\n📎 ${attachments.map(f => f.name).join(', ')}` : ''));
  input.value = '';
  autosize();

  // Заголовок чата — по первому сообщению.
  if (activeChat && messages.filter(m => m.role === 'user').length === 1) {
    activeChat.title = text.slice(0, 40) + (text.length > 40 ? '…' : '');
    await invoke('update_chat', { meta: activeChat }).catch(console.error);
    renderChatList();
  }

  // Контейнеры для ответа.
  currentAcc = '';
  reasoningAcc = '';
  thinkBody = null;
  currentAiBubble = null;

  try {
    // Убеждаемся, что модель чата загружена (на случай внешнего сбоя).
    await invoke('ensure_model_loaded', { model: activeChat.model }).catch(showError);
    await invoke('chat', { messages, webSearch, attachments });
    attachments = [];
    renderAttachBar();
  } catch (e) {
    showToast(typeof e === 'string' ? e : (e.message || 'Не получилось отправить запрос'));
  }
}

function setBusyUI(state) {
  btnSend.disabled = state;
  btnSend.textContent = state ? 'Лама думает…' : 'Отправить';
  btnStop.style.display = state ? 'inline-block' : 'none';
  btnWeb.disabled = state;
  // «Продолжить» доступен, когда не заняты и есть ответ модели в истории.
  const canContinue = !state && messages.some((m) => m.role === 'assistant');
  btnContinue.style.display = canContinue ? 'inline-block' : 'none';
}

// «Продолжить» (как в оригинале): модель продолжает последний ответ.
btnContinue.onclick = async () => {
  if (busy) return;
  const last = messages[messages.length - 1];
  if (!last || last.role !== 'assistant') return;
  busy = true;
  setBusyUI(true);
  // Ответ продолжается в том же пузыре (последний assistant остаётся
  // в messages — с continue_final_message сервер допишет его).
  currentAcc = last.content;
  reasoningAcc = '';
  thinkBody = null;
  // Переиспользуем существующий пузырь ответа, если он есть в DOM.
  const bubbles = chatEl.querySelectorAll('.msg.ai .bubble');
  currentAiBubble = bubbles.length && bubbles[bubbles.length - 1].textContent === last.content
    ? bubbles[bubbles.length - 1]
    : addMessage('assistant', currentAcc);
  addCaret(currentAiBubble);
  try {
    const gp = await invoke('get_settings').catch(() => ({ gen_params: {} }));
    const gen = { ...(gp.gen_params || {}), __continue: true };
    await invoke('chat', { messages, webSearch: false, attachments: null, genParams: gen });
  } catch (e) {
    showToast(typeof e === 'string' ? e : (e.message || 'Ошибка продолжения'));
  }
};

// ── События бэкенда ─────────────────────────────────────────
let currentAiBubble = null;
let currentAcc = '';
let reasoningAcc = '';
let thinkBody = null;
let lastUsage = null;
let statsEnabled = true;

function ensureAnswerBubble() {
  if (!currentAiBubble) {
    currentAiBubble = addMessage('assistant', '');
    addCaret(currentAiBubble);
  }
}

listen('chat://token', (e) => {
  ensureAnswerBubble();
  currentAcc += e.payload;
  removeCaret();
  currentAiBubble.textContent = currentAcc;
  addCaret(currentAiBubble);
  chatEl.scrollTop = chatEl.scrollHeight;
});

listen('chat://reasoning', (e) => {
  if (!thinkBody) thinkBody = addThinkingBlock();
  reasoningAcc += e.payload;
  thinkBody.textContent = reasoningAcc;
  chatEl.scrollTop = chatEl.scrollHeight;
});

listen('chat://usage', (e) => {
  // Статистика: usage из последнего чанка стрима.
  const u = e.payload || {};
  lastUsage = { ...lastUsage, ...u };
});

listen('chat://done', () => {
  removeCaret();
  // При «Продолжить» assistant уже есть в messages — обновим его.
  if (currentAcc.trim()) {
    if (messages.length && messages[messages.length - 1].role === 'assistant') {
      messages[messages.length - 1].content = currentAcc;
    } else {
      messages.push({ role: 'assistant', content: currentAcc });
    }
  }
  // Статистика под ответом (если включена и есть данные).
  if (statsEnabled && lastUsage && (lastUsage.completion_tokens || lastUsage.prompt_tokens)) {
    const pt = lastUsage.prompt_tokens || 0;
    const ct = lastUsage.completion_tokens || 0;
    const t = lastUsage.timings || {};
    const tps = t.predicted_per_second ? t.predicted_per_second.toFixed(1) :
                 (t.predicted_ms && ct ? (ct / (t.predicted_ms / 1000)).toFixed(1) : null);
    const stat = document.createElement('div');
    stat.className = 'stats';
    let html = '';
    if (pt) html += `<span>Промпт: <b>${pt}</b> ток.</span>`;
    if (ct) html += `<span>Ответ: <b>${ct}</b> ток.</span>`;
    if (tps) html += `<span>Скорость: <b>${tps}</b> ток./с</span>`;
    stat.innerHTML = html;
    const lastAi = [...chatEl.querySelectorAll('.msg.ai .bubble-wrap')].pop();
    if (lastAi && stat.innerHTML) lastAi.appendChild(stat);
  }
  lastUsage = null;
  currentAiBubble = null;
  currentAcc = '';
  reasoningAcc = '';
  thinkBody = null;
  busy = false;
  setBusyUI(false);
  persistHistory();
});

listen('chat://sources', (e) => addSources(e.payload.sources));

listen('chat://error', (e) => {
  showToast(String(e.payload));
  removeCaret();
  busy = false;
  setBusyUI(false);
});

listen('srv://status', (e) => {
  const s = e.payload;
  if (s.running) {
    dot.className = 'dot ok';
    statusText.textContent = 'Модель готова · ' + (s.model || '').split('/').pop().split(':')[0];
    $('btn-restart').disabled = false;
  } else if (s.starting) {
    dot.className = 'dot wait';
    statusText.textContent = s.last_log || 'Загрузка модели…';
    $('btn-restart').disabled = true;
  } else {
    dot.className = 'dot err';
    statusText.textContent = 'Сервер остановлен';
    $('btn-restart').disabled = false;
  }
  // Показываем модель активного чата под списком.
  renderSideModel();
});

// ── Кнопки ──────────────────────────────────────────────────
btnSend.onclick = send;
btnStop.onclick = () => invoke('chat_abort').catch(console.error);

btnWeb.onclick = () => {
  webSearch = !webSearch;
  btnWeb.classList.toggle('on', webSearch);
  btnWeb.textContent = (webSearch ? '✓ ' : '') + 'Поиск в интернете';
};

// ── Прикрепление файлов ─────────────────────────────────────
const fileInput = $('file-input');
const attachBar = $('attach-bar');
$('btn-file').onclick = () => fileInput.click();

fileInput.onchange = async () => {
  for (const file of fileInput.files) {
    if (attachments.some((a) => a.name === file.name)) continue;
    try {
      const path = file.path || null;
      let content;
      if (path) {
        const r = await invoke('read_text_file', { path });
        content = r.content;
      } else {
        content = await file.text();
        if (content.length > 120000) content = content.slice(0, 84000) + '\n\n[…файл сокращён…]\n\n' + content.slice(-36000);
      }
      attachments.push({ name: file.name, content });
    } catch (e) {
      showToast('«' + file.name + '»: ' + String(e));
    }
  }
  fileInput.value = '';
  renderAttachBar();
};

function renderAttachBar() {
  attachBar.innerHTML = '';
  attachBar.style.display = attachments.length ? 'flex' : 'none';
  for (const a of attachments) {
    const chip = document.createElement('div');
    chip.className = 'attach-chip';
    const nm = document.createElement('span');
    nm.textContent = '📎 ' + a.name;
    const rm = document.createElement('span');
    rm.className = 'rm';
    rm.textContent = '✕';
    rm.title = 'Убрать файл';
    rm.onclick = () => {
      attachments = attachments.filter((x) => x !== a);
      renderAttachBar();
    };
    chip.appendChild(nm);
    chip.appendChild(rm);
    attachBar.appendChild(chip);
  }
}

$('btn-restart').onclick = async () => {
  await invoke('stop_server').catch(console.error);
  await invoke('start_server').catch(console.error);
};

input.addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter') {
    if (uiSettings.send_on_enter) {
      if (!ev.shiftKey) {
        ev.preventDefault();
        send();
      }
    } else if (ev.key === 'Enter' && (ev.metaKey || ev.ctrlKey)) {
      ev.preventDefault();
      send();
    }
  }
});

// Вставка длинного текста — превращается в прикреплённый файл (как в оригинале).
input.addEventListener('paste', (ev) => {
  const text = (ev.clipboardData || window.clipboardData).getData('text');
  const limit = uiSettings.paste_long_text_to_file_len || 2500;
  if (text && text.length > limit) {
    ev.preventDefault();
    attachments.push({ name: 'вставка-' + new Date().toISOString().slice(11, 19) + '.txt', content: text });
    renderAttachBar();
    showToast('Длинный текст сохранён как файл (' + text.length + ' символов)');
  }
});

// Автоскролл: уважаем настройку.
function scrollToBottom() {
  if (uiSettings.disable_auto_scroll) return;
  chatEl.scrollTop = chatEl.scrollHeight;
}

// Примеры из экрана приветствия
document.querySelectorAll('.example').forEach((el) => {
  el.onclick = () => { input.value = el.textContent; input.focus(); autosize(); };
});

// ── UI-настройки (оригинальные General/Display) ────────────
let uiSettings = {
  send_on_enter: true,
  show_system_message: false,
  paste_long_text_to_file_len: 2500,
  title_use_first_line: true,
  title_use_llm: false,
  title_generation_prompt: '',
  theme: 'dark',
  show_message_stats: true,
  show_thought_in_progress: true,
  render_user_markdown: true,
  render_thinking_markdown: true,
  full_height_code_blocks: false,
  disable_auto_scroll: false,
  show_model_quantization: true,
  show_model_org_name: false,
  show_build_version: false,
  pdf_as_image: false,
  max_image_mpixels: 0,
  custom_css: '',
  show_raw_output_switch: false,
};

function applyUiSettings(u) {
  Object.assign(uiSettings, u || {});
  // Тема и кастомный CSS.
  applyTheme(uiSettings.theme);
  let styleEl = document.getElementById('custom-css');
  if (!styleEl) {
    styleEl = document.createElement('style');
    styleEl.id = 'custom-css';
    document.head.appendChild(styleEl);
  }
  styleEl.textContent = uiSettings.custom_css || '';
  document.body.classList.toggle('full-code', !!uiSettings.full_height_code_blocks);
  document.body.classList.toggle('no-autoscroll', !!uiSettings.disable_auto_scroll);
  statsEnabled = !!uiSettings.show_message_stats;
}

// ── Настройки ───────────────────────────────────────────────
const modal = $('modal-settings');
const genFields = [
  { id: 'gen-temperature', key: 'temperature', fmt: (v) => Number(v).toFixed(2) },
  { id: 'gen-n_predict', key: 'n_predict', fmt: (v) => (v < 0 ? 'без ограничения' : v) },
  { id: 'gen-top_k', key: 'top_k', fmt: (v) => (v < 0 ? 'выкл.' : v) },
  { id: 'gen-top_p', key: 'top_p', fmt: (v) => Number(v).toFixed(2) },
  { id: 'gen-min_p', key: 'min_p', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-typical_p', key: 'typical_p', fmt: (v) => (v >= 1 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-top_n_sigma', key: 'top_n_sigma', fmt: (v) => (v < 0 ? 'выкл.' : Number(v).toFixed(1)) },
  { id: 'gen-seed', key: 'seed', fmt: (v) => (v < 0 ? 'случайно' : v) },
  { id: 'gen-repeat_penalty', key: 'repeat_penalty', fmt: (v) => (v == 1 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-repeat_last_n', key: 'repeat_last_n', fmt: (v) => (v < 0 ? 'весь контекст' : v) },
  { id: 'gen-presence_penalty', key: 'presence_penalty', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-frequency_penalty', key: 'frequency_penalty', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-dry_multiplier', key: 'dry_multiplier', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-dry_base', key: 'dry_base', fmt: (v) => Number(v).toFixed(2) },
  { id: 'gen-dry_allowed_length', key: 'dry_allowed_length', fmt: (v) => v },
  { id: 'gen-dry_penalty_last_n', key: 'dry_penalty_last_n', fmt: (v) => (v < 0 ? 'весь контекст' : v) },
  { id: 'gen-dynatemp_range', key: 'dynatemp_range', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(1)) },
  { id: 'gen-dynatemp_exponent', key: 'dynatemp_exponent', fmt: (v) => Number(v).toFixed(1) },
  { id: 'gen-xtc_probability', key: 'xtc_probability', fmt: (v) => (v == 0 ? 'выкл.' : Number(v).toFixed(2)) },
  { id: 'gen-xtc_threshold', key: 'xtc_threshold', fmt: (v) => Number(v).toFixed(2) },
  { id: 'gen-mirostat', key: 'mirostat', fmt: (v) => (v == 0 ? 'выкл.' : v) },
  { id: 'gen-mirostat_tau', key: 'mirostat_tau', fmt: (v) => Number(v).toFixed(1) },
  { id: 'gen-mirostat_eta', key: 'mirostat_eta', fmt: (v) => Number(v).toFixed(2) },
  { id: 'gen-n_keep_messages', key: 'n_keep_messages', fmt: (v) => (v == 0 ? 'весь диалог' : v) },
];

// Сэмплеры: имена как в llama.cpp + человеческие подписи.
const SAMPLER_NAMES = {
  'penalties': 'Штрафы (repeat/presence/frequency/DRY)',
  'dry': 'DRY (анти-повторы)',
  'top_n_sigma': 'Top N Sigma',
  'top_k': 'Top K',
  'typ_p': 'Typical P',
  'top_p': 'Top P',
  'min_p': 'Min P',
  'xtc': 'XTC',
  'temperature': 'Температура',
};
const ALL_SAMPLERS = ['penalties', 'dry', 'top_n_sigma', 'top_k', 'typ_p', 'top_p', 'min_p', 'xtc', 'temperature'];
let samplersEnabled = [...ALL_SAMPLERS]; // порядок = порядок применения

function renderSamplers() {
  const box = $('samplers-list');
  box.innerHTML = '';
  const order = samplersEnabled.concat(ALL_SAMPLERS.filter((s) => !samplersEnabled.includes(s)));
  order.forEach((name, i) => {
    const on = samplersEnabled.includes(name);
    const row = document.createElement('div');
    row.className = 'sampler-row' + (on ? '' : ' off');
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = on;
    cb.onchange = () => {
      if (cb.checked) samplersEnabled.push(name);
      else samplersEnabled = samplersEnabled.filter((x) => x !== name);
      renderSamplers();
    };
    const nm = document.createElement('span');
    nm.className = 's-name';
    nm.textContent = SAMPLER_NAMES[name] || name;
    const hint = document.createElement('span');
    hint.className = 's-hint';
    hint.textContent = on ? '' : '(выключен, идёт в конец)';
    const up = document.createElement('span');
    up.className = 's-up';
    up.textContent = '↑';
    up.onclick = () => {
      if (i > 0 && on) {
        samplersEnabled.splice(i, 1);
        samplersEnabled.splice(i - 1, 0, name);
        renderSamplers();
      }
    };
    const down = document.createElement('span');
    down.className = 's-down';
    down.textContent = '↓';
    down.onclick = () => {
      if (i < samplersEnabled.length - 1 && on) {
        samplersEnabled.splice(i, 1);
        samplersEnabled.splice(i + 1, 0, name);
        renderSamplers();
      }
    };
    row.appendChild(cb);
    row.appendChild(nm);
    row.appendChild(hint);
    row.appendChild(up);
    row.appendChild(down);
    box.appendChild(row);
  });
}

function readGenFromUI() {
  const g = {};
  for (const f of genFields) g[f.key] = Number($(f.id).value);
  g.system_prompt = $('gen-system_prompt').value;
  g.samplers = [...samplersEnabled];
  g.backend_sampling = $('gen-backend_sampling').checked;
  g.timings_per_token = $('gen-timings_per_token').checked;
  g.enable_thinking = $('gen-enable_thinking').checked;
  g.disable_reasoning_parsing = $('gen-disable_reasoning_parsing').checked;
  g.exclude_reasoning_from_context = $('gen-exclude_reasoning_from_context').checked;
  g.pre_fill_kv_cache = $('gen-pre_fill_kv_cache').checked;
  g.custom_json = $('gen-custom_json').value;
  return g;
}
function writeGenToUI(g) {
  if (!g) g = {};
  const defaults = { samplers: [...ALL_SAMPLERS], backend_sampling: false, timings_per_token: false,
    enable_thinking: true, disable_reasoning_parsing: false, exclude_reasoning_from_context: false,
    pre_fill_kv_cache: false, custom_json: '' };
  g = { ...defaults, ...g };
  for (const f of genFields) $(f.id).value = g[f.key] ?? $(f.id).value;
  samplersEnabled = Array.isArray(g.samplers) && g.samplers.length ? [...g.samplers] : [...ALL_SAMPLERS];
  $('gen-backend_sampling').checked = !!g.backend_sampling;
  $('gen-timings_per_token').checked = !!g.timings_per_token;
  $('gen-enable_thinking').checked = g.enable_thinking !== false;
  $('gen-disable_reasoning_parsing').checked = !!g.disable_reasoning_parsing;
  $('gen-exclude_reasoning_from_context').checked = !!g.exclude_reasoning_from_context;
  $('gen-pre_fill_kv_cache').checked = !!g.pre_fill_kv_cache;
  $('gen-custom_json').value = g.custom_json || '';
  $('gen-system_prompt').value = g.system_prompt || '';
  renderSamplers();
  updateLabels();
}
function updateLabels() {
  for (const f of genFields) {
    const lbl = $('lbl-' + f.key);
    if (lbl) lbl.textContent = f.fmt(Number($(f.id).value));
  }
}
for (const f of genFields) {
  $(f.id).addEventListener('input', updateLabels);
}

let savedModel = ''; // не используется после переноса выбора модели в сайдбар

const uiToggles = [
  'ui-send_on_enter', 'ui-show_system_message', 'ui-title_use_first_line', 'ui-title_use_llm',
  'ui-show_thought_in_progress', 'ui-show_message_stats', 'ui-render_user_markdown',
  'ui-render_thinking_markdown', 'ui-full_height_code_blocks', 'ui-disable_auto_scroll',
  'ui-show_model_quantization', 'ui-show_model_org_name', 'ui-show_build_version',
  'ui-pdf_as_image', 'ui-show_raw_output_switch',
];

function readUiFromUI() {
  const u = {};
  for (const id of uiToggles) u[id.replace('ui-', '')] = $(id).checked;
  u.paste_long_text_to_file_len = Number($('ui-paste_long_text_to_file_len').value);
  u.title_generation_prompt = $('ui-title_generation_prompt').value;
  u.max_image_mpixels = Number($('ui-max_image_mpixels').value);
  u.custom_css = $('ui-custom_css').value;
  u.theme = $('set-theme').value;
  // Радио заголовка: один из двух.
  u.title_use_first_line = $('ui-title_use_first_line').checked;
  u.title_use_llm = $('ui-title_use_llm').checked;
  return u;
}

function writeUiToUI(u) {
  if (!u) return;
  for (const id of uiToggles) {
    const key = id.replace('ui-', '');
    if (key in u) $(id).checked = !!u[key];
  }
  $('ui-paste_long_text_to_file_len').value = u.paste_long_text_to_file_len ?? 2500;
  $('lbl-paste_len').textContent = $('ui-paste_long_text_to_file_len').value;
  $('ui-title_generation_prompt').value = u.title_generation_prompt || '';
  $('ui-max_image_mpixels').value = u.max_image_mpixels ?? 0;
  $('lbl-max_mpx').textContent = Number($('ui-max_image_mpixels').value).toFixed(1);
  $('ui-custom_css').value = u.custom_css || '';
  $('set-theme').value = u.theme || 'dark';
}

$('ui-paste_long_text_to_file_len').addEventListener('input', () => {
  $('lbl-paste_len').textContent = $('ui-paste_long_text_to_file_len').value;
});
$('ui-max_image_mpixels').addEventListener('input', () => {
  $('lbl-max_mpx').textContent = Number($('ui-max_image_mpixels').value).toFixed(1);
});

$('btn-settings').onclick = async () => {
  const [s] = await Promise.all([
    invoke('get_settings').catch(() => null),
    refreshModels(),
  ]);
  $('set-web-default').checked = !!(s && s.web_search_default);
  if (s) {
    if (s.gen_params) writeGenToUI(s.gen_params);
    writeUiToUI(s.ui);
  } else {
    writeGenToUI(null);
  }
  modal.classList.add('show');
};

$('set-cancel').onclick = () => modal.classList.remove('show');
$('set-reset').onclick = () => {
  // Дефолты llama-server (из /props).
  writeGenToUI({
    temperature: 1.0, top_k: -1, top_p: 0.95, min_p: 0.05, typical_p: 1.0,
    top_n_sigma: -1, seed: -1, n_predict: -1, repeat_penalty: 1.0, repeat_last_n: 64,
    presence_penalty: 0.0, frequency_penalty: 0.0, dry_multiplier: 0.0, dry_base: 1.75,
    dry_allowed_length: 2, dry_penalty_last_n: 64, dynatemp_range: 0.0, dynatemp_exponent: 1.0,
    xtc_probability: 0.0, xtc_threshold: 0.1, mirostat: 0, mirostat_tau: 5.0, mirostat_eta: 0.1,
    n_keep_messages: 0, system_prompt: '',
    samplers: [...ALL_SAMPLERS],
    backend_sampling: false, timings_per_token: false, enable_thinking: true, custom_json: '',
  });
};

$('set-save').onclick = async () => {
  // Глобальные настройки: генерация + UI.
  const settings = {
    model: activeChat ? activeChat.model : '',
    web_search_default: $('set-web-default').checked,
    gen_params: readGenFromUI(),
    ui: readUiFromUI(),
  };
  await invoke('save_settings', { settings }).catch((e) => showToast(String(e)));
  applyUiSettings(settings.ui);
  modal.classList.remove('show');
};

// ── Тема ─────────────────────────────────────────────────────
function applyTheme(theme) {
  document.body.dataset.theme = theme === 'light' ? 'light' : 'dark';
}

// ── Утилиты ─────────────────────────────────────────────────
function showToast(text) {
  const t = $('toast');
  t.textContent = text;
  t.classList.add('show');
  clearTimeout(showToast._t);
  showToast._t = setTimeout(() => t.classList.remove('show'), 5000);
}

function autosize() {
  input.style.height = 'auto';
  input.style.height = Math.min(input.scrollHeight, 180) + 'px';
}
input.addEventListener('input', autosize);

// ── Старт ───────────────────────────────────────────────────
(async () => {
  try {
    const s = await invoke('get_settings').catch(() => null);
    if (s) {
      if (s.web_search_default) btnWeb.click();
      applyUiSettings(s.ui);
    }
    // Модели для селекта внизу слева.
    await refreshModels();
    // Чаты: последний — активный.
    await loadChats();
    await switchToChat(chats.length ? chats[0].id : null);
  } catch (e) {
    console.error(e);
  }
  input.focus();
})();
