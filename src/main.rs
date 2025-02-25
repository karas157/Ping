// Імпортуємо необхідні бібліотеки
use eframe::{egui, NativeOptions};
use egui::{Color32, RichText, ScrollArea, Vec2};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use surge_ping::{Client, Config, PingIdentifier, PingSequence};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task;
// use tokio::time::sleep; // Видалено, бо не використовується
use chrono::Local;
use rand::random;

/// Головна структура додатку, що містить стан інтерфейсу та логіку пінгу
struct PingApp {
    // Поля для вводу параметрів
    target: String,        // Адреса цілі для пінгу
    count: String,         // Кількість запитів
    interval: String,      // Інтервал між запитами в секундах
    timeout: String,       // Час очікування відповіді в секундах

    // Результати та стан
    ping_results: Arc<Mutex<Vec<PingResult>>>,  // Список результатів пінгу, доступний між потоками
    is_pinging: bool,      // Прапор, що вказує, чи виконується пінг зараз
    status_message: String, // Повідомлення про стан виконання

    // Асинхронне середовище
    runtime: Runtime,      // Середовище виконання Tokio для асинхронних завдань
    stop_sender: Option<mpsc::Sender<()>>,  // Канал для зупинки процесу пінгу

    // Статистика
    stats: PingStats,      // Загальна статистика пінгу
}

/// Структура для зберігання результату одного пінг-запиту
struct PingResult {
    timestamp: String,     // Час виконання запиту
    target: String,        // Ціль запиту (домен або IP)
    sequence: u16,         // Номер послідовності пакету
    rtt: Option<f64>,      // Час відгуку в мілісекундах (якщо є)
    response: String,      // Статус відповіді (успіх, помилка тощо)
}

/// Структура для зберігання загальної статистики пінгу
struct PingStats {
    sent: u16,             // Кількість відправлених пакетів
    received: u16,         // Кількість отриманих відповідей
    min_time: f64,         // Мінімальний час відгуку
    max_time: f64,         // Максимальний час відгуку
    avg_time: f64,         // Середній час відгуку
}

// Реалізація значень за замовчуванням для додатку
impl Default for PingApp {
    fn default() -> Self {
        Self {
            target: "google.com".to_string(),  // Ціль за замовчуванням
            count: "10".to_string(),           // 10 запитів за замовчуванням
            interval: "1".to_string(),         // 1 секунда інтервалу за замовчуванням
            timeout: "2".to_string(),          // 2 секунди таймауту за замовчуванням
            ping_results: Arc::new(Mutex::new(Vec::new())),  // Порожній список результатів
            is_pinging: false,                 // Пінг не виконується на початку
            status_message: "Готовий до запуску".to_string(),  // Початковий статус
            runtime: Runtime::new().unwrap(),  // Створюємо середовище виконання Tokio
            stop_sender: None,                 // Немає активного каналу зупинки
            stats: PingStats {                 // Пуста статистика
                sent: 0,
                received: 0,
                min_time: f64::MAX,
                max_time: 0.0,
                avg_time: 0.0,
            },
        }
    }
}

// Реалізація інтерфейсу додатку
impl eframe::App for PingApp {
    // Метод оновлення інтерфейсу, який викликається при кожному кадрі
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Створюємо центральну панель
        egui::CentralPanel::default().show(ctx, |ui| {
            // Заголовок додатку
            ui.heading("Утиліта Ping");

            ui.add_space(10.0);  // Додаємо відступ

            // === Форма для вводу параметрів ===
            ui.horizontal(|ui| {
                ui.label("Ціль:");
                ui.text_edit_singleline(&mut self.target);  // Поле для вводу адреси
            });

            ui.horizontal(|ui| {
                ui.label("Кількість запитів:");
                ui.text_edit_singleline(&mut self.count);

                ui.label("Інтервал (сек):");
                ui.text_edit_singleline(&mut self.interval);

                ui.label("Таймаут (сек):");
                ui.text_edit_singleline(&mut self.timeout);
            });

            ui.add_space(10.0);  // Додаємо відступ

            // === Кнопки управління ===
            ui.horizontal(|ui| {
                // Якщо пінг не виконується, показуємо кнопку "Запустити"
                if !self.is_pinging {
                    if ui.button("Запустити").clicked() {
                        self.start_ping();  // Запускаємо пінг при натисканні
                    }
                } else {
                    // Інакше показуємо кнопку "Зупинити"
                    if ui.button("Зупинити").clicked() {
                        self.stop_ping();  // Зупиняємо пінг при натисканні
                    }
                }

                // Кнопка очищення результатів
                if ui.button("Очистити").clicked() {
                    self.clear_results();
                }
            });

            ui.add_space(5.0);  // Додаємо відступ

            // === Статус виконання ===
            ui.horizontal(|ui| {
                ui.label("Статус:");
                ui.label(&self.status_message);
            });

            ui.add_space(10.0);  // Додаємо відступ

            // === Статистика пінгу ===
            ui.collapsing("Статистика", |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Відправлено: {}", self.stats.sent));
                    ui.label(format!("Отримано: {}", self.stats.received));

                    // Обчислюємо відсоток втрачених пакетів, якщо є отримані відповіді
                    if self.stats.received > 0 {
                        let loss_percent = ((self.stats.sent as f64 - self.stats.received as f64) / self.stats.sent as f64) * 100.0;
                        ui.label(format!("Втрачено: {:.1}%", loss_percent));
                    }
                });

                // Показуємо статистику часу відгуку, якщо є отримані відповіді
                if self.stats.received > 0 {
                    ui.horizontal(|ui| {
                        ui.label(format!("Мінімальний час: {:.2} мс", self.stats.min_time));
                        ui.label(format!("Середній час: {:.2} мс", self.stats.avg_time));
                        ui.label(format!("Максимальний час: {:.2} мс", self.stats.max_time));
                    });
                }
            });

            ui.add_space(10.0);  // Додаємо відступ

            // === Таблиця результатів ===
            ui.label("Результати пінгу:");

            // Створюємо прокручувану область для таблиці результатів
            ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                egui::Grid::new("ping_results")
                    .num_columns(5)
                    .spacing([10.0, 4.0])
                    .striped(true)  // Смугасте оформлення таблиці
                    .show(ui, |ui| {
                        // Заголовки таблиці
                        ui.label(RichText::new("Час").strong());
                        ui.label(RichText::new("Ціль").strong());
                        ui.label(RichText::new("Послідовність").strong());
                        ui.label(RichText::new("Затримка").strong());
                        ui.label(RichText::new("Статус").strong());
                        ui.end_row();  // Завершуємо рядок заголовків

                        // Виводимо всі результати
                        let results = self.ping_results.lock().unwrap();
                        for result in results.iter() {
                            ui.label(&result.timestamp);
                            ui.label(&result.target);
                            ui.label(format!("{}", result.sequence));

                            // Відображаємо час відгуку з кольоровим індикатором або прочерк
                            match result.rtt {
                                Some(rtt) => {
                                    // Визначаємо колір залежно від часу відгуку
                                    let color = if rtt < 100.0 {
                                        Color32::GREEN  // Зелений для швидких відповідей
                                    } else if rtt < 200.0 {
                                        Color32::YELLOW  // Жовтий для середніх
                                    } else {
                                        Color32::RED  // Червоний для повільних
                                    };
                                    ui.label(RichText::new(format!("{:.2} мс", rtt)).color(color));
                                },
                                None => {
                                    ui.label("-");  // Немає часу відгуку
                                }
                            }

                            ui.label(&result.response);
                            ui.end_row();  // Завершуємо рядок даних
                        }
                    });
            });
        });

        // Запитуємо оновлення інтерфейсу, якщо пінг активний
        if self.is_pinging {
            ctx.request_repaint();
        }
    }
}

// Реалізація методів додатку
impl PingApp {
    /// Запускає процес пінгу з поточними параметрами
    fn start_ping(&mut self) {
        // Перевіряємо, чи не виконується вже пінг
        if self.is_pinging {
            return;
        }

        // === Парсимо параметри форми ===
        let target = self.target.clone();
        // Перевіряємо, чи вказана ціль
        if target.is_empty() {
            self.status_message = "Помилка: вкажіть адресу цілі".to_string();
            return;
        }

        // Парсимо кількість запитів
        let count = match self.count.parse::<u16>() {
            Ok(n) if n > 0 => n,
            _ => {
                self.status_message = "Помилка: кількість запитів має бути додатнім числом".to_string();
                return;
            }
        };

        // Парсимо інтервал
        let interval = match self.interval.parse::<u64>() {
            Ok(n) if n > 0 => n,
            _ => {
                self.status_message = "Помилка: інтервал має бути додатнім числом".to_string();
                return;
            }
        };

        // Парсимо таймаут
        let timeout = match self.timeout.parse::<u64>() {
            Ok(n) if n > 0 => n,
            _ => {
                self.status_message = "Помилка: таймаут має бути додатнім числом".to_string();
                return;
            }
        };

        // Очищуємо попередні результати і оновлюємо статус
        self.clear_results();
        self.is_pinging = true;
        self.status_message = format!("Пінг {}...", target);

        // Створюємо канал для сигналу зупинки
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        self.stop_sender = Some(stop_tx);

        // Створюємо клон Arc для передачі в задачу
        let results_arc = Arc::clone(&self.ping_results);

        // Запускаємо асинхронну задачу пінгу
        self.runtime.spawn(async move {
            // Розв'язуємо адресу
            let addr = match resolve_host(&target).await {
                Ok(a) => a,
                Err(e) => {
                    // Записуємо помилку розв'язання імені в результати
                    let timestamp = Local::now().format("%H:%M:%S").to_string();
                    let mut results = results_arc.lock().unwrap();
                    results.push(PingResult {
                        timestamp,
                        target: target.clone(),
                        sequence: 0,
                        rtt: None,
                        response: format!("Помилка: {}", e),
                    });
                    return;
                }
            };

            // Створюємо клієнта для пінгу
            let config = Config::default();
            let client = match Client::new(&config) {
                Ok(c) => c,
                Err(e) => {
                    // Записуємо помилку створення клієнта в результати
                    let timestamp = Local::now().format("%H:%M:%S").to_string();
                    let mut results = results_arc.lock().unwrap();
                    results.push(PingResult {
                        timestamp,
                        target: target.clone(),
                        sequence: 0,
                        rtt: None,
                        response: format!("Помилка створення клієнта: {}", e),
                    });
                    return;
                }
            };

            // Створюємо буфер даних для пінгу
            let payload = vec![0; 56];  // 56 байт - стандартний розмір для пінга

            // Змінні для збору статистики
            let mut success_count = 0;
            let mut total_time = 0.0;
            let mut min_time = f64::MAX;
            let mut max_time: f64 = 0.0;

            // Починаємо цикл пінгу
            let mut sequence: u16 = 0;
            'ping_loop: for _ in 0..count {
                // Перевіряємо сигнал зупинки
                if stop_rx.try_recv().is_ok() {
                    break 'ping_loop;  // Виходимо з циклу при отриманні сигналу зупинки
                }

                let timestamp = Local::now().format("%H:%M:%S").to_string();

                // Створюємо пінгер з випадковим ідентифікатором
                // Метод повертає безпосередньо Pinger, а не Result
                let mut pinger = client.pinger(addr, PingIdentifier(random())).await;

                // Встановлюємо таймаут для пінгера
                pinger.timeout(Duration::from_secs(timeout));

                // Відправляємо пінг
                match pinger.ping(PingSequence(sequence), &payload).await {
                    Ok((_, rtt)) => {
                        // Успішний пінг, обчислюємо час у мілісекундах
                        let rtt_ms = rtt.as_secs_f64() * 1000.0;

                        // Оновлюємо статистику
                        success_count += 1;
                        total_time += rtt_ms;
                        min_time = min_time.min(rtt_ms);
                        max_time = max_time.max(rtt_ms);

                        // Записуємо успішний результат
                        let mut results = results_arc.lock().unwrap();
                        results.push(PingResult {
                            timestamp,
                            target: target.clone(),
                            sequence,
                            rtt: Some(rtt_ms),
                            response: "Успішно".to_string(),
                        });
                    },
                    Err(e) => {
                        // Записуємо помилку або таймаут пінгу
                        let mut results = results_arc.lock().unwrap();
                        results.push(PingResult {
                            timestamp,
                            target: target.clone(),
                            sequence,
                            rtt: None,
                            response: format!("Таймаут або помилка: {}", e),
                        });
                    }
                }

                sequence += 1;  // Збільшуємо номер послідовності

                // Якщо це не останній пінг, чекаємо вказаний інтервал
                if sequence < count {
                    match tokio::time::timeout(
                        Duration::from_secs(interval),
                        stop_rx.recv(),
                    ).await {
                        Ok(Some(_)) => break 'ping_loop, // Отримано сигнал зупинки
                        Ok(None) => break 'ping_loop,    // Канал закрито
                        Err(_) => {}, // Таймаут інтервалу (нормальна ситуація)
                    }
                }
            }

            // Створюємо структуру статистики
            let stats = PingStats {
                sent: sequence,
                received: success_count,
                min_time: if success_count > 0 { min_time } else { 0.0 },
                max_time,
                avg_time: if success_count > 0 { total_time / success_count as f64 } else { 0.0 },
            };

            // Оновлюємо статистику в основному потоці
            task::spawn_blocking(move || {
                let mut results = results_arc.lock().unwrap();

                // Додаємо підсумок результатів
                let timestamp = Local::now().format("%H:%M:%S").to_string();
                let loss_percent = ((sequence as f64 - success_count as f64) / sequence as f64) * 100.0;
                let summary = format!(
                    "--- Статистика для {} ---\nВідправлено: {}, Отримано: {}, Втрачено: {:.1}%",
                    target, sequence, success_count, loss_percent
                );

                // Додаємо підсумковий рядок в результати
                results.push(PingResult {
                    timestamp,
                    target: target.clone(),
                    sequence: u16::MAX,  // Спеціальне значення для підсумку
                    rtt: None,
                    response: summary,
                });

                // Повертаємо статистику
                stats
            });
        });
    }

    /// Зупиняє поточний процес пінгу
    fn stop_ping(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            // Відправляємо сигнал зупинки
            let _ = sender.try_send(());
        }

        self.is_pinging = false;
        self.status_message = "Пінг зупинено".to_string();
    }

    /// Очищує результати та статистику пінгу
    fn clear_results(&mut self) {
        let mut results = self.ping_results.lock().unwrap();
        results.clear();  // Очищуємо список результатів

        // Скидаємо статистику до початкових значень
        self.stats = PingStats {
            sent: 0,
            received: 0,
            min_time: f64::MAX,
            max_time: 0.0,
            avg_time: 0.0,
        };
    }
}

/// Асинхронна функція для розв'язання імені хоста в IP-адресу
async fn resolve_host(host: &str) -> Result<IpAddr, Box<dyn std::error::Error>> {
    // Спробуємо спочатку як IP-адресу
    if let Ok(ip) = IpAddr::from_str(host) {
        return Ok(ip);  // Якщо це вже IP, повертаємо його
    }

    // Інакше пробуємо розв'язати як доменне ім'я
    let ips = tokio::net::lookup_host(format!("{}:0", host))
        .await?
        .map(|addr| addr.ip())
        .collect::<Vec<IpAddr>>();

    // Перевіряємо, чи знайдено хоча б одну IP-адресу
    if ips.is_empty() {
        return Err(format!("Не вдалося розв'язати ім'я хоста: {}", host).into());
    }

    // Повертаємо першу знайдену IP-адресу
    Ok(ips[0])
}

/// Головна функція програми
fn main() -> Result<(), eframe::Error> {
    // Налаштування вікна додатку
    let options = NativeOptions {
        initial_window_size: Some(Vec2::new(800.0, 600.0)),  // Початковий розмір вікна
        ..Default::default()
    };

    // Запускаємо додаток
    eframe::run_native(
        "Утиліта Ping",  // Заголовок вікна
        options,
        Box::new(|_cc| Box::new(PingApp::default())),  // Створюємо екземпляр додатку
    )
}