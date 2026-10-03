/// Идентификаторы событий и команд (общие для Rust и UI).

pub mod ev {
    /// Статус сервера (см. server::ServerStatus).
    pub const STATUS: &str = "srv://status";
    /// Новый токен ответа модели (строка).
    pub const TOKEN: &str = "chat://token";
    /// Токен «размышлений» reasoning-модели (строка).
    pub const REASONING: &str = "chat://reasoning";
    /// Ответ завершён.
    pub const DONE: &str = "chat://done";
    /// Источники веб-поиска (JSON: { sources: [...] }).
    pub const SOURCES: &str = "chat://sources";
    /// Статистика генерации (usage: токены и тайминги).
    pub const USAGE: &str = "chat://usage";
    /// Ошибка (строка).
    pub const ERROR: &str = "chat://error";
}
