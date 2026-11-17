//! Сервер, клиент, состояние сервера и резервный сервер.

use super::*;

impl App {
    pub(crate) fn port(&mut self) -> Option<u16> {
        match self.cfg.port.trim().parse::<u16>() {
            Ok(p) if p > 0 => Some(p),
            _ => {
                self.ui.net_status = "Неверный порт".into();
                None
            }
        }
    }

    pub(crate) fn toggle_server(&mut self, ctx: &egui::Context) {
        if self.net.server.take().is_some() {
            self.net.collector = None;
            self.net.local_health = None;
            self.net.server_online = false;
            self.net.clients = 0;
            self.ui.net_status = "Сервер остановлен".into();
            return;
        }
        let Some(port) = self.port() else { return };
        self.disconnect();
        self.save_cfg();
        self.ui.net_status = "Запуск сервера…".into();
        let ctx = ctx.clone();
        self.net.server = Some(net::start_server(
            port,
            self.cfg.password.clone(),
            self.work_dir.clone(),
            self.net.srv_tx.clone(),
            move || ctx.request_repaint(),
        ));
    }

    pub(crate) fn disconnect(&mut self) {
        self.net.lost = None;
        self.net.reconnect = None;
        if self.net.client.take().is_some() {
            self.net.connected = false;
            self.net.remote = StateMsg::default();
            self.render.previews.clear();
        }
        // Сеанс оператора закончился: «Показать курсор» включалось на том сервере
        // и не должно остаться на этом окне, когда оно само станет сервером.
        self.reset_show_cursor();
    }

    /// Запустить сборщик показателей этого компьютера (на сервере).
    pub(crate) fn start_collector(&mut self) {
        let Some(server) = &self.net.server else { return };
        let publish = server.health_publisher();
        let (tx, ctx) = (self.bus.tx.clone(), self.ctx.clone());
        self.net.collector = Some(health::start(
            self.work_dir.clone(),
            self.net.ui_frames.clone(),
            self.ppt.host.lag_probe(),
            move |h| {
                publish(&h); // клиентам — напрямую из потока сбора, без перерисовки окон
                let _ = tx.send(Out::Health(h));
            },
            move || ctx.request_repaint(),
        ));
    }

    /// Оценка состояния сервера: у клиента — показатели сервера и связь, на сервере — свои.
    pub(crate) fn health_rows(&self) -> (health::Level, Vec<health::Row>) {
        let rows = if self.net.connected {
            // Старый сервер (без пинга) молчит между изменениями — молчание тогда не тревога.
            let silent = self.net.last_seen.filter(|_| self.net.remote_live).map(|t| t.elapsed());
            // Пинг отвечает, а показатели не приходят дольше 7 с — программа на сервере зависла целиком.
            let mut h = self.net.remote_health.clone();
            if let (Some(h), Some(at)) = (h.as_mut(), self.net.health_at) {
                if at.elapsed() > Duration::from_secs(7) {
                    h.ui_ok = false;
                }
            }
            if self.net.remote_live { health::assess(h.as_ref(), self.net.rtt, silent) } else { Vec::new() }
        } else {
            health::assess(self.net.local_health.as_ref(), None, None)
        };
        (health::overall(&rows), rows)
    }

    /// Загрузчик на резервный сервер (создаётся при первой надобности, заново — если резерв сменили).
    pub(crate) fn mirror_uploader(&mut self) -> Option<&net::Uploader> {
        let srv = self.reserve_server().filter(|_| self.cfg.mirror)?;
        if self.net.mirror.as_ref().is_none_or(|(k, _)| *k != srv.key()) {
            let port = srv.port.trim().parse().ok()?;
            let ctx = self.ctx.clone();
            let up = net::start_uploader(srv.ip.trim().to_string(), port, srv.password.clone(), self.net.mirror_tx.clone(), move || ctx.request_repaint());
            self.net.mirror = Some((srv.key(), up));
        }
        self.net.mirror.as_ref().map(|(_, u)| u)
    }

    /// Резервный сервер из списка, если он выбран и это не тот, к которому мы уже подключены.
    pub(crate) fn reserve_server(&self) -> Option<SavedServer> {
        let current = format!("{}:{}", self.cfg.ip.trim(), self.cfg.port.trim());
        self.cfg.servers.iter().find(|s| s.key() == self.cfg.reserve && s.key() != current).cloned()
    }

    /// Отключиться от текущего сервера и подключиться к резервному.
    pub(crate) fn switch_to_reserve(&mut self) {
        let Some(srv) = self.reserve_server() else { return };
        // Что было на экране основного сервера — откроем то же на резерве (по имени файла).
        // Список сервера при обрыве уже очищен — берём то, что запомнили.
        self.net.resume = self.session.last_show.clone().map(|(name, page)| Resume { name, page, opened: None, since: Instant::now() });
        self.net.mirror = None; // резерв станет основным
        self.disconnect();
        self.ui.status = format!("Переключение на резервный сервер {}…", srv.label());
        (self.cfg.ip, self.cfg.port, self.cfg.password, self.cfg.name) = (srv.ip, srv.port, srv.password, srv.name);
        let ctx = self.ctx.clone();
        self.toggle_client(&ctx);
    }

    /// Продолжение показа на резерве: найти файл с тем же именем, открыть, перейти на тот же слайд.
    pub(crate) fn continue_resume(&mut self) {
        let Some(r) = &mut self.net.resume else { return };
        if r.since.elapsed() > Duration::from_secs(20) {
            self.ui.status = format!("На резервном сервере не удалось открыть «{}»", r.name);
            self.net.resume = None;
            return;
        }
        match r.opened {
            None => {
                let Some(it) = self.net.remote.items.iter().find(|i| i.name == r.name) else { return };
                if !it.ready {
                    return;
                }
                let id = it.id;
                r.opened = Some(id);
                self.act(Cmd::Open { id });
            }
            Some(id) if self.net.remote.active == Some(id) => {
                let (page, name) = (r.page, r.name.clone());
                self.net.resume = None;
                if page > 0 && self.net.remote.video.is_none() {
                    self.act(Cmd::GoTo { page });
                }
                self.ui.status = format!("На резервном сервере открыт «{name}», слайд {}", page + 1);
            }
            Some(_) => {}
        }
    }

    /// Подтянуть в список серверы, которые запомнила другая копия программы после нашего запуска.
    pub(crate) fn merge_servers_from_disk(&mut self) {
        let disk: Option<Config> = std::fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str(&s).ok());
        merge_servers(&mut self.cfg.servers, disk.map(|c| c.servers).unwrap_or_default(), &self.session.forgotten);
    }

    /// Запомнить текущий сервер (адрес, порт, пароль, название) первым в списке.
    pub(crate) fn remember_server(&mut self) {
        let srv = SavedServer {
            name: self.cfg.name.trim().to_string(),
            ip: self.cfg.ip.trim().to_string(),
            port: self.cfg.port.trim().to_string(),
            password: self.cfg.password.clone(),
        };
        let key = srv.key();
        self.session.forgotten.remove(&key);
        self.cfg.servers.retain(|s| s.key() != key);
        self.cfg.servers.insert(0, srv);
        self.cfg.servers.truncate(MAX_SERVERS);
        self.save_cfg();
    }

    /// Сервер из окна «Резервный сервер» — в список сохранённых и сразу резервным.
    /// Такой адрес уже есть — обновляем его (название, пароль) на том же месте.
    pub(crate) fn save_reserve(&mut self, srv: SavedServer) {
        let srv = SavedServer { name: srv.name.trim().to_string(), ip: srv.ip.trim().to_string(), port: srv.port.trim().to_string(), password: srv.password };
        let key = srv.key();
        self.session.forgotten.remove(&key);
        upsert_server(&mut self.cfg.servers, srv);
        self.cfg.reserve = key;
        self.save_cfg();
    }

    pub(crate) fn toggle_client(&mut self, ctx: &egui::Context) {
        if self.net.client.is_some() {
            self.disconnect();
            self.ui.net_status = "Отключено".into();
            return;
        }
        let Some(port) = self.port() else { return };
        if self.net.server.is_some() {
            self.ui.net_status = "Сначала остановите сервер в этом окне".into();
            return;
        }
        self.save_cfg();
        self.ui.net_status = "Подключение…".into();
        let ctx = ctx.clone();
        self.net.client = Some(net::start_client(
            self.cfg.ip.trim().to_string(),
            port,
            self.cfg.password.clone(),
            self.net.cli_tx.clone(),
            move || ctx.request_repaint(),
        ));
    }

    // ------------------------------------------------------------ интерфейс
}
