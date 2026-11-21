# murmur — дизайн

Лёгкая личная «экосистема» между Android-телефоном и несколькими ноутбуками
Arch/Hyprland через домашний сервер-релей. Без общей LAN, без VPN, без
внешних координаторов. Сервер не понимает содержимое.

## Компоненты

| Компонент      | Язык  | Роль |
|----------------|-------|------|
| `murmur-proto` | Rust  | Общий крейт: конверт, крипто, типы сообщений. Используется сервером и murmurd. |
| `murmur-server`| Rust  | Тупой WSS-релей: auth по токену, маршрутизация фреймов, реестр устройств. Live-only, без очередей. |
| `murmurd`      | Rust  | Демон ноута: WS-клиент + подсистемы (уведомления, MPRIS, буфер, presence). |
| `murmurctl`    | Rust  | CLI ноута: pair, status, devices, revoke. Вся настройка ноута — терминал. |
| `murmur` (app) | Kotlin| Android-агент: foreground service, источник событий, приёмник команд. |

## Топология

```
 phone ──WSS──► murmur-server ◄──WSS── murmurd × N
```

Все устройства держат постоянные исходящие WSS-соединения к серверу.
Сервер видит только: кто подключён, к кому фрейм, размер и тайминг.

## Идентичность и auth

- `DeviceId` — 8 байт случайности, hex-строка. Генерируется при первом запуске.
- `AuthToken` — 32 байта случайности на устройство. Сервер хранит только
  `SHA-256(token)` в `devices.json` рядом с бинарём:
  `{ device_id: { name, token_hash, revoked } }`.
- После WS-коннекта устройство шлёт `Hello{device_id, token}`. Сервер проверяет
  хэш и `revoked`, привязывает сокет к device_id. `from` во фреймах сервер
  перезаписывает — подделать чужой id нельзя.

## Крипто

- **Семейный ключ** `K` — 32 байта, один на все устройства, раздаётся при пейринге.
- Payload каждого фрейма: `XChaCha20-Poly1305(K, nonce=random24, plaintext)`.
- Отзыв устройства: `revoked=true` в реестре (отсекает auth) + ротация `K`
  на оставшихся (повторная раздача по pairwise-каналу… упрощение v1: ручная
  смена ключа на оставшихся устройствах; автоматическую ротацию делаем позже).

## Пейринг (invite-код + SAS)

1. На существующем устройстве `murmurctl invite` генерирует session id и
   эфемерный X25519-ключ, выводит invite-бандл `{server_url, inviter_id,
   session}` — QR или строка. Сервер сессий не знает: матчмейкинга нет.
2. Новое устройство (`murmurd pair --code …` или скан в приложении)
   подключается `role=pair`, заявляя session как свой conn-id, генерирует
   свой эфемерный ключ и шлёт `PairHello{pubkey, name}` → inviter'у.
   Pair-коннекты изолированы: только Pair*-фреймы, только unicast.
3. `murmurctl pair` показывает pending + `SAS = 6 цифр из
   H(sorted pkA‖pkB ‖ sorted inviter‖session)` — симметричен, обе стороны
   считают одинаково. `pair accept` шлёт `PairAccept{pubkey}` — теперь код
   виден и на новом устройстве; человек сверяет два экрана. Сервер,
   подменивший ключи, даст разные коды.
4. `pair confirm`: inviter генерирует `device_id` + `AuthToken`, шлёт
   серверу `RegisterDevice{device_id, name, token_hash}`, а новому
   устройству — `PairData{blob}`: `{device_id, token, family_key,
   server_url}` под XChaCha20-Poly1305 от DH-ключа. Новое устройство
   само пишет config+secrets и переподключается уже authed.

## Wire protocol

WS-бинарные фреймы. Конверт (открытым текстом, его видит сервер):

```json
{ "v": 1, "from": "a1b2c3d4e5f60708", "to": "*" | "b2…" | "@server",
  "seq": 1234, "payload": "<base64 ciphertext>" }
```

`to="*"` — fan-out всем живым устройствам семьи, кроме отправителя.
`to="@server"` — control-plane (Hello, RegisterDevice, RevokeDevice,
ListDevices, PairHello, PairAccept), payload без семейного ключа.

Внутри шифротекста — типизированное сообщение (serde enum):

| Тип                    | Направление          | Содержимое |
|------------------------|----------------------|------------|
| `Presence`             | любой → `*`          | `{state: active|idle|locked, ts}` |
| `Notification`         | phone → `*`          | `{key, pkg, title, body, icon?, ts}` |
| `NotificationDismiss`  | любой → `*`          | `{key}` |
| `MediaState`           | phone → `*`          | `{player, title, artist, album, status, pos_ms, dur_ms, art?}` |
| `MediaCommand`         | laptop → phone       | `{op: play|pause|next|prev|seek(pos_ms)}` |
| `Clipboard`            | любой → см. правила  | `{origin, hash, mime, data_b64, explicit, ts}` |
| `KeyRotate`            | любой authed → `*`   | новый `K` (при отзыве устройства) |

`seq` монотонен на отправителя; получатель отбрасывает `(from, seq)` старше
уже виденного — защита от дубликатов при реконнекте.

## Presence и «активный узел»

murmurd вычисляет локальное состояние:

- `locked` — `logind` LockedHint или экран заблокирован;
- `idle` — `ext-idle-notify-v1` сработал (порог ~3 мин, конфиг);
- `active` — иначе.

При смене состояния шлёт `Presence` на `*`. Каждый узел (и телефон) хранит
карту `{device_id → (state, ts)}`. Активный = узел в состоянии `active` с
максимальным `ts` перехода в active; ничья — меньший device_id. Все узлы с
одинаковой картой приходят к одинаковому ответу — координатор не нужен.

## Буфер обмена

- Ноут→ноут: любое локальное изменение → `Clipboard` на `*`, применяют все.
- Ноут→телефон: тот же `Clipboard`, но на телефон его шлёт **только узел,
  считающий себя активным** (`to=phone`).
- Телефон→ноуты: `Clipboard{explicit=true}` на `*` по явному действию
  (share / process-text); применяет только активный ноут («отправить на тот,
  за которым сижу»). Ноуты шлют `explicit=false` — применяют все.
- Анти-эхо: узел хранит `last_hash` применённого/отправленного. Watcher при
  входящем `hash == last_hash` игнорирует; при применении удалённого значения
  выставляет `expected_hash`, чтобы свой же watcher не зациклил отправку.
- Типы: `text/plain`, `text/uri-list`, `image/png`. Файлы — v2.
- Секреты: не отправлять, если **любой** MIME в offer попадает в denylist
  (`clipboard_mime_deny`, дефолт — `x-kde-passwordManagerHint`; менеджеры
  паролей мечут клипы этим маркером рядом с `text/plain`). Wayland не
  сообщает app-источник буфера — per-app denylist только через косвенный
  хак с `activewindow`, не делаем в v1.

## Уведомления

- Phone `NotificationListenerService` → `Notification` на `*` (фильтр
  пакетов — allowlist/denylist в настройках приложения).
- murmurd → `org.freedesktop.Notifications.Notify` (swaync), хранит маппинг
  `phone_key ↔ local_nid`.
- `NotificationClosed(nid)` от swaync → `NotificationDismiss{key}` на `*`;
  телефон `cancelNotification`; другие ноуты закрывают свой `nid`.
  Закрытие «по команде из сети» выставляет флаг, чтобы не ответить на свой же
  `NotificationClosed` (анти-петля).

## Медиа

- Phone: `MediaSessionManager` → `MediaState` на `*` при смене метаданных/
  статуса; позиция — при seek и каждые ~5с во время воспроизведения.
- murmurd: MPRIS-интерфейс `org.mpris.MediaPlayer2.murmur` (zbus). Телефон
  выглядит обычным плеером: медиаклавиши, playerctl, waybar/swaync-виджеты.
- Позиция экстраполируется локально (`pos + rate×elapsed`), `Seeked` при
  апдейтах. `Play/Pause/Next/Previous/Seek` → `MediaCommand` телефону.
- Обложка: байты в `MediaState.art`, murmurd кладёт в
  `~/.cache/murmur/art.<hash>` и отдаёт `file://` как `mpris:artUrl`.

## Надёжность

- Реконнект: экспоненциальный backoff 1с→60с + jitter; на Android —
  немедленный retry по `ConnectivityManager` callback при смене сети.
- Live-only: события, отправленные пока узел оффлайн, теряются. При
  реконнекте узел шлёт `Presence` и запрашивает свежий `MediaState`.
- Телефон: foreground service + `POST_NOTIFICATIONS` + исключение из
  battery optimization (`ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`).

## Управление — только терминал

Один паттерн везде: у каждого демона — unix-сокет админки с
newline-JSON, CLI ходит туда. Файлы конфигурации/реестров — только
persistence, а не шов (один писатель — сам демон).

- `murmur-server`: `add-device --name X` (bootstrap первого устройства —
  выдаёт device_id+token), `list-devices`, `revoke --id X`. Сокет:
  `$XDG_RUNTIME_DIR/murmur-server.sock` (`MURMUR_ADMIN_SOCK` для override).
  Revoke убивает живой коннект мгновенно.
- `murmurd init --server … --name … --device-id … --token …` — провижининг
  ноута: пишет config.toml + secrets.toml (0600), семейный ключ генерится
  локально и никогда не покидает устройства.
- `murmurctl` → `$XDG_RUNTIME_DIR/murmur.sock`: `status`, `devices`,
  `revoke`, `invite` (QR + код), `pair` / `pair accept` / `pair confirm`,
  `send`.
- Конфиг ноута: `~/.config/murmur/config.toml` (server url, device name,
  пороги idle, denylist MIME, allowlist-политики).
- Секреты ноута (family key, auth token):
  `~/.local/share/murmur/secrets.toml`, mode 0600.

## Ограничения v1 / осознанные упрощения

- Нет offline-очередей, истории уведомлений, inline-ответов, файлов.
- Ротация `K` при отзыве — протокол заложен (`KeyRotate`), UX упрощён.
- Per-app denylist буфера — только MIME-маркеры + sensitive-флаг.

## Репозиторий

```
DESIGN.md
server/                # murmur-server — релей (самостоятельный cargo-проект)
laptop/                # cargo workspace ноута
  murmurd/             #   демон
  murmurctl/           #   CLI
mobile/                # Android-приложение (Kotlin)
shared/
  proto/               # murmur-proto — конверт, крипто, типы (path-dep из server и laptop)
```

Модульность murmurd: один conn-таск + по таску на фичу. Фича — модуль с
`feature() -> Feature` (фильтр входящих + `run(ctx)`), своё состояние — у себя.
Реестр — `registry()` в main.rs: добавление фичи = новый файл + одна строка.
Хочешь говорить с пирами — новый вариант `Message` в murmur-proto.

## Деплой (детали позже)

- `murmur-server`: systemd unit на домашнем сервере, WSS за Caddy/nginx
  (auto-HTTPS по домену) или rustls+acme.
- `murmurd`: `systemd --user` unit, autostart с сессией.
- Приложение: sideload APK.
