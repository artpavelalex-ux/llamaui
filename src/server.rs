//! Управление llama-server: запуск, остановка, статус, чат-стриминг.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::process::{Child, Command};

use crate::ids::ev;
use crate::SHUTTING_DOWN;

/// Модель из «команды для запуска.txt» пользователя.
pub const DEFAULT_MODEL: &str = "ItsOkayNow/Spark-X2.5-4B-Heretic-GGUF:Q4_K_M";
pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 8080;

fn llama_server_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("LLAMA_SERVER_PATH") {
        return p.into();
    }
    let home = dirs::home_dir().unwrap_or_else(|| "/Users/art".into());
    home.join(".local/bin/llama-server")
}

// ---------------------------------------------------------------------------
// Статус
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    pub starting: bool,
    pub model: String,
    /// Короткая последняя строка лога — для строки состояния UI.
    pub last_log: String,
}

pub struct ServerHandle {
    pub child: Mutex<Option<Child>>,
    pub starting: AtomicBool,
    pub started_at: Mutex<Option<Instant>>,
    pub model: Mutex<String>,
    pub log: Mutex<String>,
    pub abort_chat: Arc<AtomicBool>,
    pub active_chats: Arc<AtomicI64>,
}

impl Default for ServerHandle {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            starting: AtomicBool::new(false),
            started_at: Mutex::new(None),
            model: Mutex::new(DEFAULT_MODEL.into()),
            log: Mutex::new(String::new()),
            abort_chat: Arc::new(AtomicBool::new(false)),
            active_chats: Arc::new(AtomicI64::new(0)),
        }
    }
}

impl ServerHandle {
    fn status(&self) -> ServerStatus {
        let running = self.child.lock().map(|c| c.is_some()).unwrap_or(false);
        ServerStatus {
            running,
            starting: self.starting.load(Ordering::Relaxed),
            model: self.model.lock().map(|m| m.clone()).unwrap_or_default(),
            last_log: self.log.lock().map(|l| l.clone()).unwrap_or_default(),
        }
    }

    fn set_log(&self, line: impl Into<String>) {
        if let Ok(mut l) = self.log.lock() {
            *l = line.into();
        }
    }

    /// Убить llama-server: сначала синхронный SIGTERM (надёжно, даже если
    /// приложение завершается), затем асинхронный start_kill как дубль.
    pub fn kill_child(&self) {
        let pid = self.child_pid();
        if let Some(pid) = pid {
            // Синхронный сигнал — не зависит от того, жив ли ещё tokio-runtime.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.start_kill();
            }
        }
    }

    /// PID запущенного llama-server (для надёжной очистки при выходе).
    /// Важно: читается ДО kill_child, после него хранилище пусто.
    pub fn child_pid(&self) -> Option<i32> {
        if let Ok(guard) = self.child.lock() {
            guard
                .as_ref()
                .and_then(|c| c.id())
                .and_then(|pid| i32::try_from(pid).ok())
        } else {
            None
        }
    }

    fn is_alive(&self) -> bool {
        match self.child.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(c) => matches!(c.try_wait(), Ok(None)),
                None => false,
            },
            Err(_) => false,
        }
    }
}

fn emit_status(app: &AppHandle) {
    let st = app.state::<ServerHandle>();
    let _ = app.emit(ev::STATUS, st.status());
}

// ---------------------------------------------------------------------------
// Запуск / остановка
// ---------------------------------------------------------------------------

fn port_open(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

#[tauri::command]
pub async fn start_server(app: AppHandle, st: State<'_, ServerHandle>) -> Result<(), String> {
    start_server_inner(app, &st).await
}

/// Переключить модель под конкретный чат: если на сервере другая —
/// останавливаем сервер, меняем модель, запускаем заново (модель выгружается
/// вместе с завершением процесса llama-server).
#[tauri::command]
pub async fn ensure_model_loaded(app: AppHandle, st: State<'_, ServerHandle>, model: String) -> Result<bool, String> {
    let current = st
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default();
    let active_ok = st.is_alive() || st.starting.load(Ordering::Relaxed);

    if current == model && active_ok {
        // Модель уже та — ничего делать не надо.
        return Ok(false);
    }
    if current == model && !active_ok {
        // Модель верная, сервер не поднят — поднимаем.
        start_server_inner(app.clone(), &st).await?;
        return Ok(true);
    }

    // Другая модель: глушим сервер (модель выгружается из памяти),
    // меняем модель и стартуем заново.
    if let Ok(mut m) = st.model.lock() {
        *m = model;
    }
    if active_ok {
        st.kill_child();
        // Ждём освобождения порта (модель выгружается вместе с процессом).
        let mut waited = 0u32;
        while port_open(PORT) {
            if waited >= 60 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            waited += 1;
        }
    }
    start_server_inner(app.clone(), &st).await?;
    Ok(true)
}

pub async fn start_server_inner(app: AppHandle, st: &ServerHandle) -> Result<(), String> {
    if st.is_alive() || st.starting.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Живой сервер уже на порту — сразу «готово», ничего не поднимаем.
    if port_open(PORT) && external_server_ready().await {
        st.starting.store(false, Ordering::Relaxed);
        st.set_log("Сервер уже запущен (внешний)");
        emit_status(&app);
        return Ok(());
    }

    st.starting.store(true, Ordering::Relaxed);
    st.set_log("Запуск сервера…");
    emit_status(&app);

    // Логи сервера пишем в один файл (stdout и stderr — клоны одного FD).
    let logfile = std::env::temp_dir().join("lama-chat-server.log");
    let log_out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&logfile)
        .map(std::process::Stdio::from)
        .unwrap_or(std::process::Stdio::null());
    let log_err = match std::fs::OpenOptions::new()
        .write(true)
        .truncate(false)
        .open(&logfile)
    {
        Ok(f) => std::process::Stdio::from(f),
        Err(_) => std::process::Stdio::null(),
    };

    let model = st
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.into());

    // Порт занят? Разбираемся: живой внешний сервер / умирающий наш / чужой процесс.
    if port_open(PORT) {
        let mut waited = 0u32;
        loop {
            // Живой сервер отвечает 200 — подключаемся к нему.
            if external_server_ready().await {
                st.starting.store(false, Ordering::Relaxed);
                st.set_log("Подключено к работающему серверу");
                emit_status(&app);
                return Ok(());
            }
            // Порт освободился (наш старый процесс умер) — запускаем свой.
            if !port_open(PORT) {
                break;
            }
            if waited >= 30 {
                st.starting.store(false, Ordering::Relaxed);
                st.set_log("Порт 8080 занят другим процессом. Закройте его или поменяйте порт.");
                emit_status(&app);
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            waited += 1;
        }
    }

    let mut cmd = Command::new(llama_server_path());
    // Локальный .gguf — флаг -m; репозиторий HuggingFace — флаг -hf.
    let is_local_file = model.ends_with(".gguf") && std::path::Path::new(&model).exists();
    if is_local_file {
        cmd.arg("-m").arg(&model);
    } else {
        cmd.arg("-hf").arg(&model);
    }
    cmd.arg("--host").arg(HOST)
        .arg("--port").arg(PORT.to_string())
        .stdout(log_out)
        .stderr(log_err)
        .kill_on_drop(true);

    match cmd.spawn() {
        Ok(c) => *st.child.lock().unwrap() = Some(c),
        Err(e) => {
            st.starting.store(false, Ordering::Relaxed);
            let msg = format!("Не удалось запустить llama-server: {e}");
            st.set_log(msg.clone());
            emit_status(&app);
            let _ = app.emit(ev::ERROR, msg);
            return Ok(());
        }
    }

    // Фоновая задача: ждём готовности / следим за смертью процесса.
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        wait_ready(app2).await;
    });

    Ok(())
}

/// Живой ли сервер на порту (200 от /health)?
async fn external_server_ready() -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();
    let url = format!("http://{HOST}:{PORT}/health");
    matches!(
        client.get(&url).send().await,
        Ok(r) if r.status().is_success()
    )
}

/// Ожидание готовности: опрашиваем /health, читаем хвост лога.
async fn wait_ready(app: AppHandle) {
    let st = app.state::<ServerHandle>();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    let url = format!("http://{HOST}:{PORT}/health");
    let logfile = std::env::temp_dir().join("lama-chat-server.log");

    let start = Instant::now();
    loop {
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return;
        }
        // Готово?
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                st.starting.store(false, Ordering::Relaxed);
                st.set_log("Готов к работе");
                emit_status(&app);
                return;
            }
        }
        // Процесс умер?
        if !st.is_alive() {
            st.starting.store(false, Ordering::Relaxed);
            let tail = last_log_line(&logfile);
            st.set_log(format!(
                "Сервер остановился{}",
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            ));
            emit_status(&app);
            let _ = app.emit(
                ev::ERROR,
                format!("Сервер не запустился. Последняя строка лога: {tail}"),
            );
            return;
        }
        // Прогресс: последняя строка лога.
        let tail = last_log_line(&logfile);
        if !tail.is_empty() {
            let shown = if tail.len() > 160 {
                format!("…{}", &tail[tail.len() - 157..])
            } else {
                tail
            };
            st.set_log(shown);
            emit_status(&app);
        }
        // Таймаут: 30 минут (модель может качаться долго).
        if start.elapsed() > Duration::from_secs(1800) {
            st.set_log("Слишком долго. Проверьте интернет-соединение.");
            emit_status(&app);
            return;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

fn last_log_line(path: &std::path::Path) -> String {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[tauri::command]
pub fn stop_server(app: AppHandle, st: State<'_, ServerHandle>) -> Result<(), String> {
    st.kill_child();
    st.starting.store(false, Ordering::Relaxed);
    if let Ok(mut g) = st.started_at.lock() {
        *g = None;
    }
    st.set_log("Сервер остановлен");
    emit_status(&app);
    Ok(())
}

#[tauri::command]
pub fn get_status(st: State<'_, ServerHandle>) -> ServerStatus {
    st.status()
}

// ---------------------------------------------------------------------------
// Чат
// ---------------------------------------------------------------------------

/// Один запрос к модели: стримит токены в UI.
/// web_search=true добавляет поиск, attachments — текст прикреплённых файлов.
#[tauri::command]
pub async fn chat(
    app: AppHandle,
    st: State<'_, ServerHandle>,
    messages: Vec<Value>,
    web_search: bool,
    attachments: Option<Vec<Value>>,
    gen_params: Option<Value>,
) -> Result<(), String> {
    use futures_util::StreamExt;

    st.abort_chat.store(false, Ordering::Release);
    st.active_chats.fetch_add(1, Ordering::AcqRel);

    let mut msgs = messages;

    // 0) Прикреплённые файлы: содержимое уходит последнему вопросу как контекст.
    if let Some(files) = attachments {
        let named: Vec<String> = files
            .iter()
            .filter_map(|f| {
                let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("файл");
                let content = f.get("content").and_then(|c| c.as_str()).unwrap_or("");
                if content.is_empty() {
                    None
                } else {
                    Some(format!("=== Файл: {name} ===\n{content}"))
                }
            })
            .collect();
        if !named.is_empty() {
            let joined = named.join("\n\n");
            // Прикрепляем к последнему сообщению пользователя.
            if let Some(last_user) = msgs
                .iter_mut()
                .rev()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            {
                let old = last_user
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                last_user["content"] = serde_json::json!(format!(
                    "{old}\n\nК пользователю прикреплён файл (используй его содержимое при ответе):\n{joined}"
                ));
            }
        }
    }

    // 1) Веб-поиск: находим контекст. System-вставки собираем ОТДЕЛЬНО и
    // ставим в НАЧАЛО диалога: строгие шаблоны (Qwen и др.) требуют
    // system только первым сообщением, иначе Jinja-ошибка 500.
    let mut system_parts: Vec<String> = vec![];
    if web_search {
        let last_user = msgs
            .iter()
            .rev()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            .find_map(|m| {
                m.get("content").and_then(|c| c.as_str()).map(|s| s.to_string())
            });
        if let Some(q) = last_user {
            match crate::websearch::search(&q).await {
                Ok(res) => {
                    let _ = app.emit(ev::SOURCES, serde_json::json!({ "sources": res.results }));
                    if !res.results.is_empty() {
                        let ctx = res
                            .results
                            .iter()
                            .map(|r| format!("• {} — {}\n  {}", r.title, r.snippet, r.url))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        system_parts.push(format!(
                            "Тебе доступны результаты веб-поиска по запросу пользователя. Опирайся на них при ответе. Отвечай на русском языке. Если в результатах нет нужного — скажи об этом честно.\n\nРезультаты поиска:\n{ctx}"
                        ));
                    }
                }
                Err(e) => {
                    let _ = app.emit(
                        ev::ERROR,
                        format!("Поиск в интернете не удался: {e}"),
                    );
                }
            }
        }
    }

    // 2) Параметры генерации: из вызова, иначе — сохранённые настройки.
    // Мержим по ключам: пришедшие перекрывают сохранённые, пробелы — из сохранённых.
    let settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default();
    let mut base = serde_json::to_value(&settings.gen_params)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if let Some(incoming) = gen_params.as_ref().and_then(|v| v.as_object()) {
        for (k, val) in incoming {
            base.insert(k.clone(), val.clone());
        }
    }
    let g: GenParams = serde_json::from_value(Value::Object(base)).unwrap_or_default();

    // Ограничение истории: берём только последние N сообщений.
    if g.n_keep_messages > 0 && msgs.len() > g.n_keep_messages as usize {
        let tail = msgs.split_off(msgs.len() - g.n_keep_messages as usize);
        msgs = tail;
    }

    // Системная подсказка из настроек — тоже в system_parts (первой).
    if !g.system_prompt.trim().is_empty() {
        system_parts.insert(0, g.system_prompt.trim().to_string());
    }

    // Все system-части объединяем в ОДНО сообщение строго в начале диалога
    // (строгие шаблоны Qwen/Llama не терпят system в середине истории).
    if !system_parts.is_empty() {
        // Заодно вычищаем случайные system из истории (могут остаться
        // после старых версий приложения) — вливаем их в общий блок.
        let mut extra: Vec<String> = msgs
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()).map(|s| s.to_string()))
            .collect();
        system_parts.append(&mut extra);
        msgs.retain(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"));
        let joined = system_parts.join("\n\n---\n\n");
        msgs.insert(0, serde_json::json!({
            "role": "system",
            "content": joined
        }));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("http://{HOST}:{PORT}/v1/chat/completions");
    let mut body = gen_params_to_body(&g);
    body.insert("messages".into(), json!(msgs));
    body.insert("stream".into(), json!(true));
    // Статистика токенов (как «Show message statistics» в оригинале).
    body.insert("stream_options".into(), json!({ "include_usage": true }));

    // «Продолжить» (как в оригинале): последний ответ не перегенерируется,
    // а модель продолжает его. Сигнал приходит через gen_params.__continue.
    let is_continue = gen_params
        .as_ref()
        .and_then(|v| v.get("__continue"))
        .and_then(|c| c.as_bool())
        .unwrap_or(false);
    if is_continue {
        body.insert("continue_final_message".into(), json!(true));
        body.insert("add_generation_prompt".into(), json!(false));
    }

    // Настройки размышлений (Reasoning, как в оригинале).
    if g.disable_reasoning_parsing {
        // reasoning_format=none — размышления придут внутри текста.
        body.insert("reasoning_format".into(), json!("none"));
    }
    if g.exclude_reasoning_from_context {
        // Убираем размышления из предыдущих сообщений истории.
        for m in msgs.iter_mut() {
            if m.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                if let Some(obj) = m.as_object_mut() {
                    obj.remove("reasoning_content");
                }
            }
        }
    }

    let resp = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            let err = format!("Не удалось связаться с сервером: {e}");
            let _ = app.emit(ev::ERROR, err.clone());
            st.active_chats.fetch_sub(1, Ordering::AcqRel);
            return Err(err);
        }
    };

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let txt = resp.text().await.unwrap_or_default();
        let err = format!("Сервер вернул ошибку {code}");
        let _ = app.emit(ev::ERROR, err.clone());
        let _ = txt;
        st.active_chats.fetch_sub(1, Ordering::AcqRel);
        return Err(err);
    }

    let abort = st.abort_chat.clone();
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut result: Result<(), String> = Ok(());
    while let Some(chunk) = stream.next().await {
        if abort.load(Ordering::Acquire) {
            break; // Пользователь нажал «Остановить»
        }
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                result = Err(format!("Обрыв соединения: {e}"));
                break;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..pos + 1);
            if !line.starts_with("data:") {
                continue;
            }
            let payload = line[5..].trim();
            if payload == "[DONE]" {
                buf.clear();
                break;
            }
            if let Ok(v) = serde_json::from_str::<Value>(payload) {
                if let Some(choice) = v["choices"].get(0) {
                    if let Some(delta) = choice["delta"]["content"].as_str() {
                        let _ = app.emit(ev::TOKEN, delta);
                    }
                    // Reasoning-модели шлют размышления до ответа.
                    if let Some(r) = choice["delta"]["reasoning_content"].as_str() {
                        let _ = app.emit(ev::REASONING, r);
                    }
                }
                // Статистика: последний чанк содержит usage (токены).
                if let Some(usage) = v.get("usage") {
                    if usage.is_object() {
                        let _ = app.emit(ev::USAGE, usage.clone());
                    }
                }
                // Тайминги из usage.timing_per_token (если запрошено).
                if let Some(timings) = v.get("timings") {
                    let _ = app.emit(ev::USAGE, json!({ "timings": timings }));
                }
            }
        }
    }

    st.active_chats.fetch_sub(1, Ordering::AcqRel);
    let _ = app.emit(ev::DONE, ());
    result
}

/// Остановить генерацию ответа (кнопка «Остановить»).
#[tauri::command]
pub fn chat_abort(st: State<'_, ServerHandle>) -> Result<(), String> {
    st.abort_chat.store(true, Ordering::Release);
    Ok(())
}

// ---------------------------------------------------------------------------
// Настройки
// ---------------------------------------------------------------------------

/// Параметры генерации — полный набор из веб-интерфейса llama-server (/props).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenParams {
    // Основные сэмплеры
    pub temperature: f64,
    pub top_k: i64,
    pub top_p: f64,
    pub min_p: f64,
    pub typical_p: f64,
    pub top_n_sigma: f64,
    // Динамическая температура
    pub dynatemp_range: f64,
    pub dynatemp_exponent: f64,
    // XTC (альтернатива сэмплингу)
    pub xtc_probability: f64,
    pub xtc_threshold: f64,
    // Штрафы
    pub repeat_penalty: f64,
    pub repeat_last_n: i64,
    pub presence_penalty: f64,
    pub frequency_penalty: f64,
    // DRY (анти-повторы)
    pub dry_multiplier: f64,
    pub dry_base: f64,
    pub dry_allowed_length: i64,
    pub dry_penalty_last_n: i64,
    // Mirostat
    pub mirostat: i64,
    pub mirostat_tau: f64,
    pub mirostat_eta: f64,
    // Порядок сэмплеров (как в оригинальной веб-морде)
    pub samplers: Vec<String>,
    // Сэмплирование на ускорителе (Metal)
    pub backend_sampling: bool,
    // Показывать тайминги каждого токена
    pub timings_per_token: bool,
    // Разрешить модели размышлять (thinking) — для reasoning-моделей
    pub enable_thinking: bool,
    // Произвольные параметры JSON — мерджатся в запрос (Custom JSON из оригинала)
    pub custom_json: String,
    // Reasoning (оригинальная секция Reasoning)
    pub disable_reasoning_parsing: bool,
    pub exclude_reasoning_from_context: bool,
    // Производительность (оригинальная секция Performance)
    pub pre_fill_kv_cache: bool,
    // Прочее
    pub seed: i64,
    pub n_predict: i64,
    pub n_keep_messages: u32,
    pub system_prompt: String,
}

impl Default for GenParams {
    fn default() -> Self {
        // Совпадает с дефолтами llama-server (проверено по /props).
        Self {
            temperature: 1.0,
            top_k: -1,
            top_p: 0.95,
            min_p: 0.05,
            typical_p: 1.0,
            top_n_sigma: -1.0,
            dynatemp_range: 0.0,
            dynatemp_exponent: 1.0,
            xtc_probability: 0.0,
            xtc_threshold: 0.1,
            repeat_penalty: 1.0,
            repeat_last_n: 64,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            dry_multiplier: 0.0,
            dry_base: 1.75,
            dry_allowed_length: 2,
            dry_penalty_last_n: 64,
            mirostat: 0,
            mirostat_tau: 5.0,
            mirostat_eta: 0.1,
            samplers: vec![
                "penalties".into(),
                "dry".into(),
                "top_n_sigma".into(),
                "top_k".into(),
                "typ_p".into(),
                "top_p".into(),
                "min_p".into(),
                "xtc".into(),
                "temperature".into(),
            ],
            backend_sampling: false,
            timings_per_token: false,
            enable_thinking: true,
            custom_json: String::new(),
            disable_reasoning_parsing: false,
            exclude_reasoning_from_context: false,
            pre_fill_kv_cache: false,
            seed: -1,
            n_predict: -1,
            n_keep_messages: 0,
            system_prompt: String::new(),
        }
    }
}

/// Собрать JSON-тело параметров для /v1/chat/completions.
fn gen_params_to_body(g: &GenParams) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("temperature".into(), json!(g.temperature));
    m.insert("top_k".into(), json!(g.top_k));
    m.insert("top_p".into(), json!(g.top_p));
    m.insert("min_p".into(), json!(g.min_p));
    m.insert("typical_p".into(), json!(g.typical_p));
    m.insert("top_n_sigma".into(), json!(g.top_n_sigma));
    m.insert("dynatemp_range".into(), json!(g.dynatemp_range));
    m.insert("dynatemp_exponent".into(), json!(g.dynatemp_exponent));
    m.insert("xtc_probability".into(), json!(g.xtc_probability));
    m.insert("xtc_threshold".into(), json!(g.xtc_threshold));
    m.insert("repeat_penalty".into(), json!(g.repeat_penalty));
    m.insert("repeat_last_n".into(), json!(g.repeat_last_n));
    m.insert("presence_penalty".into(), json!(g.presence_penalty));
    m.insert("frequency_penalty".into(), json!(g.frequency_penalty));
    m.insert("dry_multiplier".into(), json!(g.dry_multiplier));
    m.insert("dry_base".into(), json!(g.dry_base));
    m.insert("dry_allowed_length".into(), json!(g.dry_allowed_length));
    m.insert("dry_penalty_last_n".into(), json!(g.dry_penalty_last_n));
    m.insert("mirostat".into(), json!(g.mirostat));
    m.insert("mirostat_tau".into(), json!(g.mirostat_tau));
    m.insert("mirostat_eta".into(), json!(g.mirostat_eta));
    if !g.samplers.is_empty() {
        m.insert("samplers".into(), json!(g.samplers));
    }
    if g.backend_sampling {
        m.insert("backend_sampling".into(), json!(true));
    }
    if g.timings_per_token {
        m.insert("timings_per_token".into(), json!(true));
    }
    if !g.enable_thinking {
        // Отключаем размышления у reasoning-моделей.
        m.insert("chat_template_kwargs".into(), json!({ "enable_thinking": false }));
    }
    if g.seed >= 0 {
        m.insert("seed".into(), json!(g.seed));
    }
    if g.n_predict >= 0 {
        m.insert("max_tokens".into(), json!(g.n_predict));
    }
    // Custom JSON из оригинальной веб-морды: мерджим поверх.
    if !g.custom_json.trim().is_empty() {
        if let Ok(custom) = serde_json::from_str::<Value>(g.custom_json.trim()) {
            if let Some(obj) = custom.as_object() {
                for (k, v) in obj {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
    }
    m
}

/// Настройки вида (оригинальные вкладки General / Display).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    // General
    pub show_system_message: bool,
    pub send_on_enter: bool,
    pub paste_long_text_to_file_len: i64,
    pub title_use_first_line: bool,
    pub title_use_llm: bool,
    pub title_generation_prompt: String,
    // Display
    pub theme: String,
    pub show_message_stats: bool,
    pub show_thought_in_progress: bool,
    pub render_user_markdown: bool,
    pub render_thinking_markdown: bool,
    pub full_height_code_blocks: bool,
    pub disable_auto_scroll: bool,
    pub show_model_quantization: bool,
    pub show_model_org_name: bool,
    pub show_build_version: bool,
    // Reasoning (применяются в чате)
    // Attachments
    pub pdf_as_image: bool,
    pub max_image_mpixels: f64,
    // Developer
    pub custom_css: String,
    pub show_raw_output_switch: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            show_system_message: true,
            send_on_enter: true,
            paste_long_text_to_file_len: 2500,
            title_use_first_line: true,
            title_use_llm: false,
            title_generation_prompt: String::new(),
            theme: "dark".into(),
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
            max_image_mpixels: 0.0,
            custom_css: String::new(),
            show_raw_output_switch: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Репозиторий HF («владелец/модель:квант») ИЛИ путь к локальному .gguf.
    pub model: String,
    pub web_search_default: bool,
    pub gen_params: GenParams,
    pub ui: UiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            web_search_default: false,
            gen_params: GenParams::default(),
            ui: UiSettings::default(),
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("lama-chat").join("settings.json")
}

#[tauri::command]
pub fn get_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn save_settings(settings: Settings, st: State<'_, ServerHandle>) -> Result<(), String> {
    if let Ok(mut m) = st.model.lock() {
        *m = settings.model.clone();
    }
    let p = settings_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(
        p,
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Мульти-чаты: каждый чат — своя модель и своя история.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatMeta {
    pub id: String,
    pub title: String,
    pub model: String,
    pub created_at: i64,
}

impl Default for ChatMeta {
    fn default() -> Self {
        Self {
            id: new_chat_id(),
            title: "Новый чат".into(),
            model: DEFAULT_MODEL.into(),
            created_at: unix_now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ChatsFile {
    /// Порядок чатов в сайдбаре (новые сверху).
    pub order: Vec<String>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_chat_id() -> String {
    format!("{:x}", unix_now()) + &format!("{:04x}", rand_suffix())
}

fn rand_suffix() -> u16 {
    // Простой псевдо-случайный суффикс из времени.
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u16)
        .unwrap_or(0);
    n.wrapping_mul(0x9E37).rotate_left(4)
}

fn chats_dir() -> std::path::PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("lama-chat").join("chats")
}

fn chat_meta_path(id: &str) -> std::path::PathBuf {
    chats_dir().join(format!("{id}.json"))
}

fn chat_history_path(id: &str) -> std::path::PathBuf {
    chats_dir().join(format!("{id}.history.json"))
}

fn chats_index_path() -> std::path::PathBuf {
    chats_dir().join("_index.json")
}

/// Загрузить мету чата.
fn load_chat(id: &str) -> Option<ChatMeta> {
    std::fs::read_to_string(chat_meta_path(id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn save_chat(meta: &ChatMeta) -> Result<(), String> {
    let dir = chats_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        chat_meta_path(&meta.id),
        serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Список всех чатов (свежие первыми).
#[tauri::command]
pub fn list_chats() -> Vec<ChatMeta> {
    let idx: ChatsFile = std::fs::read_to_string(chats_index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut chats: Vec<ChatMeta> = idx
        .order
        .iter()
        .filter_map(|id| load_chat(id))
        .collect();
    if chats.is_empty() {
        // Файла нет — сканируем папку (первый запуск после обновления).
        if let Ok(entries) = std::fs::read_dir(chats_dir()) {
            let mut scanned: Vec<ChatMeta> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.ends_with(".json") && !n.starts_with('_')
                })
                .filter_map(|e| {
                    let id = e
                        .path()
                        .file_stem()?
                        .to_string_lossy()
                        .to_string();
                    load_chat(&id)
                })
                .collect();
            scanned.sort_by_key(|c| -c.created_at);
            chats = scanned;
        }
    }
    chats
}

/// Создать новый чат с указанной моделью.
#[tauri::command]
pub fn create_chat(model: Option<String>, title: Option<String>) -> Result<ChatMeta, String> {
    let mut meta = ChatMeta::default();
    if let Some(m) = model {
        meta.model = m;
    }
    if let Some(t) = title {
        meta.title = t;
    }
    save_chat(&meta)?;

    let mut idx: ChatsFile = std::fs::read_to_string(chats_index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    idx.order.insert(0, meta.id.clone());
    let dir = chats_dir();
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        chats_index_path(),
        serde_json::to_string_pretty(&idx).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(meta)
}

/// Переименовать чат (или сменить модель).
#[tauri::command]
pub fn update_chat(meta: ChatMeta) -> Result<(), String> {
    save_chat(&meta)
}

/// Удалить чат вместе с историей.
#[tauri::command]
pub fn delete_chat(id: String) -> Result<(), String> {
    let _ = std::fs::remove_file(chat_meta_path(&id));
    let _ = std::fs::remove_file(chat_history_path(&id));
    let mut idx: ChatsFile = std::fs::read_to_string(chats_index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    idx.order.retain(|x| x != &id);
    let _ = std::fs::write(
        chats_index_path(),
        serde_json::to_string_pretty(&idx).unwrap_or_else(|_| "{}".into()),
    );
    Ok(())
}

/// Загрузить историю чата (массив сообщений).
#[tauri::command]
pub fn get_chat_history(id: String) -> Vec<Value> {
    std::fs::read_to_string(chat_history_path(&id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Сохранить историю чата.
#[tauri::command]
pub fn save_chat_history(id: String, messages: Vec<Value>) -> Result<(), String> {
    let dir = chats_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        chat_history_path(&id),
        serde_json::to_string_pretty(&messages).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Выбор модели: сканируем локальные GGUF + HF-кэш
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// Что подставить в настройки: HF-репозиторий или путь к файлу.
    pub id: String,
    /// Человекочитаемое имя для списка.
    pub name: String,
    /// Откуда модель: «Скачанная (Hugging Face)», «Локальный файл» и т.п.
    pub source: String,
    /// Установлена ли сейчас (совпадает с активной).
    pub active: bool,
}

/// Найти все локальные модели: кэш llama.cpp/HF + LM Studio.
#[tauri::command]
pub fn list_models(st: State<'_, ServerHandle>) -> Vec<ModelInfo> {
    let current = st
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.into());
    let mut out: Vec<ModelInfo> = vec![];
    let mut seen: Vec<String> = vec![];

    // 1) Кэш HuggingFace (то, что скачивал llama-server через -hf).
    if let Some(home) = dirs::home_dir() {
        let hub = home.join(".cache/huggingface/hub");
        if let Ok(entries) = std::fs::read_dir(&hub) {
            let mut dirs: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("models--"))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for repo_dir in dirs {
                // models--Владелец--Модель -> Владелец/Модель
                let raw = repo_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let repo = raw
                    .strip_prefix("models--")
                    .unwrap_or(&raw)
                    .replace("--", "/");
                // Ищем .gguf в snapshots
                let snap = repo_dir.join("snapshots");
                let mut found: Option<String> = None;
                if let Ok(subs) = std::fs::read_dir(&snap) {
                    for s in subs.filter_map(|e| e.ok()) {
                        if let Ok(files) = std::fs::read_dir(s.path()) {
                            for f in files.filter_map(|e| e.ok()) {
                                let name = f.file_name().to_string_lossy().to_string();
                                if name.ends_with(".gguf") {
                                    // Квант из имени файла: *-Q4_K_M.gguf
                                    let quant = name
                                        .trim_end_matches(".gguf")
                                        .rsplit('-')
                                        .next()
                                        .unwrap_or("")
                                        .to_string();
                                    let quant = if quant.starts_with('Q')
                                        || quant.starts_with('I')
                                        || quant == "f16"
                                        || quant == "f32"
                                    {
                                        format!(":{quant}")
                                    } else {
                                        String::new()
                                    };
                                    found = Some(format!("{repo}{quant}"));
                                    break;
                                }
                            }
                        }
                        if found.is_some() { break; }
                    }
                }
                if let Some(id) = found {
                    if !seen.contains(&id) {
                        seen.push(id.clone());
                        out.push(ModelInfo {
                            active: id == current,
                            id,
                            name: raw
                                .strip_prefix("models--")
                                .unwrap_or(&raw)
                                .replace("--", " · "),
                            source: "Скачанная (Hugging Face)".into(),
                        });
                    }
                }
            }
        }

        // 2) LM Studio (~/​.lmstudio/models/Владелец/Модель/*.gguf)
        let lms = home.join(".lmstudio/models");
        if let Ok(vendors) = std::fs::read_dir(&lms) {
            for v in vendors.filter_map(|e| e.ok()) {
                if let Ok(models) = std::fs::read_dir(v.path()) {
                    for m in models.filter_map(|e| e.ok()) {
                        if let Ok(files) = std::fs::read_dir(m.path()) {
                            for f in files.filter_map(|e| e.ok()) {
                                let path = f.path();
                                if path
                                    .extension()
                                    .map(|e| e == "gguf")
                                    .unwrap_or(false)
                                {
                                    if !seen.contains(&path.to_string_lossy().to_string()) {
                                        seen.push(path.to_string_lossy().to_string());
                                        out.push(ModelInfo {
                                            id: path.to_string_lossy().to_string(),
                                            name: format!(
                                                "{} · {}",
                                                m.file_name().to_string_lossy(),
                                                f.file_name().to_string_lossy()
                                            ),
                                            source: "LM Studio".into(),
                                            active: path.to_string_lossy().to_string()
                                                == current,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Текущая модель может быть из ниоткуда (вручную вписана) — добавим.
    if !current.is_empty() && !seen.contains(&current) {
        out.push(ModelInfo {
            active: true,
            id: current.clone(),
            name: current.clone(),
            source: "Указана вручную".into(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Файлы: чтение прикреплённых текстовых файлов
// ---------------------------------------------------------------------------

/// Прочитать файл как текст (с ограничением размера). Возвращает имя + содержимое.
#[tauri::command]
pub fn read_text_file(path: String) -> Result<Value, String> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024; // 2 МБ текста — достаточно для контекста
    const MAX_SEND: usize = 120_000; // ~30k токенов

    let meta = std::fs::metadata(&path).map_err(|e| format!("Файл не найден: {e}"))?;
    if meta.len() > MAX_BYTES {
        return Err("Файл слишком большой (больше 2 МБ)".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Не удалось прочитать: {e}"))?;

    // UTF-8 или UTF-16 (BOM), иначе — латин-1 как последний шанс.
    let text = if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let (enc, _) = if bytes.starts_with(&[0xFF, 0xFE]) {
            (encoding_utf16le(&bytes), true)
        } else {
            (encoding_utf16be(&bytes), true)
        };
        enc
    } else if let Ok(s) = std::string::String::from_utf8(bytes.clone()) {
        s
    } else {
        bytes
            .iter()
            .map(|&b| b as char)
            .collect::<String>()
    };

    let text = text.trim().to_string();
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "файл".into());

    // Обрезаем с сохранением начала и конца (для длинных файлов).
    let text = if text.len() > MAX_SEND {
        let head: String = text.chars().take(MAX_SEND * 7 / 10).collect();
        let tail: String = text
            .chars()
            .rev()
            .take(MAX_SEND * 3 / 10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{head}\n\n[…файл сокращён…]\n\n{tail}")
    } else {
        text
    };

    if text.is_empty() {
        return Err("Файл пуст".into());
    }

    Ok(serde_json::json!({ "name": name, "content": text }))
}

fn encoding_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes[2..]
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn encoding_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes[2..]
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Открыть ссылку в браузере (источники поиска).
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
