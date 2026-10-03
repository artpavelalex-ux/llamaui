//! Лама Чат — десктопное приложение для локальной LLM.
//! Управляет сервером llama.cpp, показывает русский чат-интерфейс.

mod ids;
mod server;
mod websearch;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;

pub use ids::ev;
pub use server::{ServerHandle, ServerStatus};

/// Устанавливается, когда пользователь закрыл окно — глушим сервер.
pub static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .manage(server::ServerHandle::default())
        .invoke_handler(tauri::generate_handler![
            server::start_server,
            server::stop_server,
            server::get_status,
            server::chat,
            server::chat_abort,
            server::open_url,
            server::get_settings,
            server::save_settings,
            server::list_models,
            server::read_text_file,
            server::list_chats,
            server::create_chat,
            server::update_chat,
            server::delete_chat,
            server::get_chat_history,
            server::save_chat_history,
            server::ensure_model_loaded,
        ])
        .setup(|app| {
            // Автозапуск сервера при старте приложения (после отрисовки окна).
            let apph = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let st = apph.state::<server::ServerHandle>();
                server::start_server_inner(apph.clone(), &st).await.ok();
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Ловим закрытие окна (CloseRequested — до закрытия, Destroyed — подстраховка).
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                SHUTTING_DOWN.store(true, Ordering::Release);
                if let Some(st) = window.app_handle().try_state::<server::ServerHandle>() {
                    st.kill_child();
                    // Подстраховка чистильщиком: SIGKILL через 1.5 с, если SIGTERM не помог.
                    let pid = st.child_pid();
                    std::thread::spawn(move || {
                        if let Some(pid) = pid {
                            std::thread::sleep(std::time::Duration::from_millis(1500));
                            unsafe {
                                libc::kill(pid, libc::SIGKILL);
                            }
                        }
                    });
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Ошибка сборки Лама Чат")
        .run(|app_handle, event| {
            // Гарантированная точка выхода: убиваем сервер при завершении приложения.
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                SHUTTING_DOWN.store(true, Ordering::Release);
                if let Some(st) = app_handle.try_state::<server::ServerHandle>() {
                    st.kill_child();
                }
            }
        });
}

