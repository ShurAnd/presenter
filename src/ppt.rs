//! Показ PPTX через установленный PowerPoint: запускается настоящее слайд-шоу,
//! которым мы управляем (вперёд, назад, переход к слайду) и за которым следим.
//! Мост к PowerPoint — скрытый процесс PowerShell, работающий с ним по COM.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum PptEvent {
    /// Показ запущен; число слайдов.
    Ready(usize),
    /// Текущий слайд (с единицы) и всего слайдов.
    Pos(usize, usize),
    /// Показ завершён (в том числе закрыт на самом сервере).
    Ended,
    Error(String),
}

const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
function Say($s) { [Console]::Out.WriteLine($s); [Console]::Out.Flush() }
$app = $null; $pres = $null; $win = $null; $total = 0
try {
    $app = New-Object -ComObject PowerPoint.Application
    $pres = $app.Presentations.Open($env:PRES_SRC, -1, 0, 0)
    $total = $pres.Slides.Count
    $ss = $pres.SlideShowSettings
    if ($env:PRES_WINDOWED -eq '1') { $ss.ShowType = 2 } else { $ss.ShowType = 1 }
    try { $ss.ShowPresenterView = 0 } catch {}
    $win = $ss.Run()
    Say ('ready ' + $total)
} catch {
    Say ('error ' + ($_.Exception.Message -replace '\s+', ' '))
}
if ($win) {
    while ($true) {
        $line = [Console]::In.ReadLine()
        if ($line -eq $null -or $line -eq 'close') { break }
        try {
            $view = $win.View
            $cur = $view.CurrentShowPosition
            if ($line -eq 'next') {
                # На последнем шаге последнего слайда не идём дальше, иначе показ закроется.
                $last = $false
                if ($cur -ge $total) {
                    try { $last = ($view.GetClickIndex() -ge $view.GetClickCount()) } catch { $last = $true }
                }
                if (-not $last) { $view.Next() }
            } elseif ($line -eq 'prev') {
                $view.Previous()
            } elseif ($line -like 'goto *') {
                $view.GotoSlide([int]$line.Substring(5))
            } elseif ($line -eq 'show') {
                # Сцена с фоном отпустила экран — выводим слайд-шоу на передний план.
                try { $win.Activate() } catch {}
            }
            Say ('pos ' + $view.CurrentShowPosition + ' ' + $total)
        } catch {
            # PowerPoint мог быть просто занят; показ окончен, только если его окна больше нет.
            $alive = $false
            try { $alive = ($app.SlideShowWindows.Count -gt 0) } catch {}
            if (-not $alive) { Say 'ended'; $win = $null; break }
        }
    }
}
try { if ($win) { $win.View.Exit() } } catch {}
try { if ($pres) { $pres.Close() } } catch {}
try { if ($app -and $app.Presentations.Count -eq 0) { $app.Quit() } } catch {}
"#;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}

/// Пока жив этот объект — идёт показ. При уничтожении показ закрывается.
pub struct Ppt {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    done: Arc<AtomicBool>,
}

impl Ppt {
    pub fn start(src: &Path, windowed: bool, on_event: impl Fn(PptEvent) + Send + 'static) -> Result<Ppt, String> {
        // PRESENTATOR_PPT_CMD — подмена моста для отладки без PowerPoint.
        let mut cmd = match std::env::var_os("PRESENTATOR_PPT_CMD") {
            Some(c) => Command::new(c),
            None => {
                let mut c = Command::new("powershell");
                c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT]);
                c
            }
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .env("PRES_SRC", src)
            .env("PRES_WINDOWED", if windowed { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("не удалось запустить PowerShell: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().ok_or("нет вывода процесса")?;
        let done = Arc::new(AtomicBool::new(false));

        // Чтение событий.
        let done_r = done.clone();
        std::thread::spawn(move || {
            let mut finished = false;
            let mut started = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut it = line.trim().splitn(2, ' ');
                let (head, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                match head {
                    "ready" => {
                        started = true;
                        on_event(PptEvent::Ready(rest.trim().parse().unwrap_or(0)));
                    }
                    "pos" => {
                        let mut n = rest.split_whitespace().filter_map(|x| x.parse::<usize>().ok());
                        if let (Some(cur), Some(total)) = (n.next(), n.next()) {
                            on_event(PptEvent::Pos(cur, total));
                        }
                    }
                    "ended" => {
                        finished = true;
                        on_event(PptEvent::Ended);
                    }
                    "error" => {
                        finished = true;
                        on_event(PptEvent::Error(rest.to_string()));
                    }
                    _ => {}
                }
            }
            let _ = child.wait();
            done_r.store(true, Ordering::SeqCst);
            if !finished {
                on_event(if started { PptEvent::Ended } else { PptEvent::Error("PowerPoint не запустился".into()) });
            }
        });

        // Регулярный опрос позиции: слайды могут листать и на самом сервере.
        let poll = stdin.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(400));
            let mut g = poll.lock().unwrap_or_else(|e| e.into_inner());
            let ok = g.as_mut().is_some_and(|s| s.write_all(b"poll\n").and_then(|_| s.flush()).is_ok());
            if !ok {
                break;
            }
        });

        Ok(Ppt { stdin, done })
    }

    /// Флаг «процесс моста завершился». Нужен, чтобы не запускать новый показ,
    /// пока прежний ещё закрывает PowerPoint (иначе его Quit() убьёт новый).
    pub fn done_flag(&self) -> Arc<AtomicBool> {
        self.done.clone()
    }

    /// Вывести слайд-шоу на передний план (после того как сцена опустилась под него).
    pub fn reveal(&self) {
        // Разрешаем процессу PowerShell забрать фокус: сами мы сейчас активное окно.
        #[cfg(windows)]
        unsafe {
            AllowSetForegroundWindow(u32::MAX); // ASFW_ANY
        }
        self.send("show");
    }

    pub fn send(&self, line: &str) {
        let mut g = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            let _ = s.write_all(format!("{line}\n").as_bytes()).and_then(|_| s.flush());
        }
    }
}

impl Drop for Ppt {
    fn drop(&mut self) {
        self.send("close");
        // Закрываем канал: скрипт завершит показ и закроет презентацию.
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
