use feeds::mail::Mailer;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Semaphore,
};

pub struct Smtp {
    port: u16,
    pub commands: Arc<Mutex<Vec<String>>>,
    pub messages: Arc<Mutex<Vec<String>>>,
    pub gate: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Smtp {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Smtp {
    pub async fn new(accept: bool, hold: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let messages = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(Semaphore::new(if hold { 0 } else { 100 }));
        let (commands_task, messages_task, gate_task) =
            (commands.clone(), messages.clone(), gate.clone());
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (commands, messages, gate) = (
                    commands_task.clone(),
                    messages_task.clone(),
                    gate_task.clone(),
                );
                clients.spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut read = BufReader::new(read);
                    write
                        .write_all(b"220 smtp.example.test test fixture\r\n")
                        .await
                        .unwrap();
                    loop {
                        let mut line = String::new();
                        if read.read_line(&mut line).await.unwrap() == 0 {
                            break;
                        }
                        commands.lock().unwrap().push(line.clone());
                        let reply: &[u8] = if line.starts_with("EHLO ") {
                            b"250-smtp.example.test\r\n250 AUTH PLAIN\r\n"
                        } else if line.starts_with("AUTH PLAIN ") {
                            b"235 Authentication successful\r\n"
                        } else if line.starts_with("MAIL FROM:") || line.starts_with("RCPT TO:") {
                            b"250 OK\r\n"
                        } else if line == "DATA\r\n" {
                            write.write_all(b"354 End with a dot\r\n").await.unwrap();
                            let mut message = String::new();
                            loop {
                                let mut line = String::new();
                                assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
                                if line == ".\r\n" {
                                    break;
                                }
                                message.push_str(line.strip_prefix('.').unwrap_or(&line));
                            }
                            messages.lock().unwrap().push(message);
                            let _permit = gate.acquire().await.unwrap();
                            if accept {
                                b"250 Message accepted\r\n"
                            } else {
                                b"550 Message rejected\r\n"
                            }
                        } else if line == "QUIT\r\n" {
                            let _ = write.write_all(b"221 Bye\r\n").await;
                            break;
                        } else {
                            b"500 Unsupported\r\n"
                        };
                        if write.write_all(reply).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            port,
            commands,
            messages,
            gate,
            task,
        }
    }
    pub fn mailer(&self, mode: &str) -> Arc<Mailer> {
        Arc::new(
            Mailer::from_settings(|key| match key {
                "SMTP_HOST" => Some("127.0.0.1".into()),
                "SMTP_PORT" => Some(self.port.to_string()),
                "SMTP_TLS" => Some(mode.into()),
                "SMTP_FROM" => Some("Feeds <reset@example.test>".into()),
                "SMTP_USERNAME" => Some("synthetic-smtp-user".into()),
                "SMTP_PASSWORD" => Some("synthetic-smtp-password".into()),
                "TELESINK_PUBLIC_URL" => Some("https://events.example.test".into()),
                _ => None,
            })
            .unwrap()
            .unwrap(),
        )
    }
}
