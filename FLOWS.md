# FLOWS — все пользовательские сценарии murmur

Каждый флоу описан детерминированно: триггер → шаги → ожидаемый результат →
как проверено. Id вида `F-N` используются в тестах и живом прогоне.

Термины: **link** — валидированный TCP-туннель между двумя членами семьи
(handshake + beacon принят). **Transit** — пир достижим через чужой линк
(фреймы флудятся дальше). **Candidate** — запомненный адрес пира для
дозвона; у пира их до 4, упорядочены: первый — доказанный линком, дальше
альтернативы из хинтов (beacon advert, pair seed).

---

## 1. Паринг (вход устройства в семью)

### F-1. Ноут приглашает телефон (основной флоу)

- Триггер: `murmurctl invite` на ноуте → QR + код `XXXX-XXXX`.
- Шаги: телефон вводит код + ip ноута → открывает pair-сокет :17895 →
  ECDH → на обеих сторонах SAS `ddd ddd` → ноут
  `murmurctl pair accept/confirm` (или invite-walkthrough), телефон жмёт
  «match» → телефон регистрируется, получает family key → сервис поднимается,
  линк устанавливается по seed-адресу.
- Ожидание: `murmurctl devices` на ноуте показывает телефон `online`;
  телефон показывает ноут онлайн. Pair-порт закрывается.
- Изолированная AP: phone→laptop по wifi мёртв (ARP FAILED в обе стороны
  на тестовой сети) → pair идёт через `adb reverse tcp:17895 tcp:17895`,
  в поле ip вводится `127.0.0.1`. Дальше линк живёт на
  `adb forward tcp:27894 tcp:17894` (см. F-9).

### F-2. Телефон приглашает ноут

- Триггер: на телефоне «invite» → код. На ноуте `murmurctl pair` →
  ввод кода → SAS → confirm на телефоне.
- Ожидание: симметрично F-1. Дозвон инициирует меньший device id —
  направление сокета не влияет на результат (tie-break F-7).

### F-3. Ноут приглашает ноут (второй компьютер в семье)

- Триггер: `murmurctl invite` на первом, `murmurctl pair` на втором.
- Ожидание: оба видят друг друга; с этого момента действуют правила
  выборов активной ноды (F-13) и транзита (F-8).

### F-4. Паринг прерван (неверный SAS / таймаут / отмена)

- Шаги: на любом шаге до confirm — сторона отменяет.
- Ожидание: pending-сессия умирает по TTL; в roster фантом не попадает;
  никаких линков и редиалов на несостоявшегося пира. Повторный invite —
  новый код, старый инвалидирован.
- Известный зазор (зафиксирован 2026-11-28): если joiner завершил SAS,
  но упал на локальной записи (config уже существовал), повторный join
  **с тем же кодом** молча висит — инвайтер уже потребил сессию.
  Рекавери — свежий invite; wipe'd-устройство перепарится под новым id,
  а «same identity key re-pairs» подменяет старый id, только если ключ
  не был утерян вместе с secrets.

---

## 2. Установление и переключение транспорта

### F-5. Холодный старт: discovery по UDP-beacon

- Триггер: оба демона живы, линка нет, broadcast проходит.
- Шаги: beacons :17893/udp → DialHint → dial → handshake → link.
- Ожидание: линк в пределах одного beacon-каденса + ~секунд на handshake.

### F-6. Холодный старт без broadcast (изоляция / гость Wi-Fi)

- Триггер: broadcast не проходит, но peer в roster и есть candidates.
- Шаги: `redial_loop` каждые ~10с шлёт DialHint на **все** candidates пира;
  коннекты идут **параллельно** (каждый в своей задаче, таймаут 4с) —
  мёртвый кандидат не тормозит живого. Первый прошедший handshake линк
  и есть результат.
- Ожидание: линк ≤ ~14с худший случай. Если candidates пусты —
  `murmurctl dial ip[:port]` (manual hint, тоже без очереди).

### F-7. Одновременные дозвоны (tie-break)

- Триггер: обе стороны редиалят друг друга → два сокета на пару.
- Ожидание: обе стороны **независимо** закрывают проигравший сокет —
  выживает сокет, открытый меньшим device id (`winner_dir`). Никакой
  координации, флапов после settle нет. Dial-guard пропускает hints,
  пока winner-направление не поднято.

### F-8. Транзит: A⇄B⇄C, прямого A⇄C нет

- Сетка: телефон привязан к ноуту кабелем; комп в wifi с ноутом; wifi-
  клиенты изолированы → прямой comp⇄phone невозможен.
- Ожидание: фреймы флудятся — комп видит телефон `online` по
  `heard_recently` (любой валидный фрейм за 30с, в т.ч. периодический
  LanPing каждые 5с — пинг едет мешем как Route::All трафик).
  RTT — ICMP-семантика: каждый член отвечает на чужой пинг понгом,
  **адресованным** пингеру (`to = Route::Device`), меш маршрутизирует
  его назад; пингер меряет `now − ts` **на своих часах** от своего же
  эха — честно для прямого и транзитного пути, никакой зависимости от
  синхронизации часов. Понг в транзите не стампится нигде (его ts —
  чужие часы). `rtt_ms` = прямой RTT линка ‖ mesh-RTT с понга.
  Notification, media, clipboard работают транзитом без конфигурации.
  UI телефона симметрично помечает комп online+ms по `lastHeard`/`heardRtt`.

### F-9. ADB-forward как транспорт (тестовая среда / dev)

- Триггер: `adb forward tcp:27894 tcp:17894` на ноуте +
  `murmurctl dial 127.0.0.1:27894`.
- Ожидание: линк валидируется → `lan_addr_set` промоутит
  `127.0.0.1:27894` в начало candidates; реальный порт сокета пишется
  (не LINK_PORT) → self-dial невозможен. Phone-side: линки с
  local-addr 127.0.0.1 не дропаются `onNetworkChanged`.

### F-10. Обрыв транспорта → мгновенный фейловер

- Триггер: смерть сокета (убит `adb forward`, ушёл интерфейс, пир умер).
- Шаги: `lan_clear` снимает линк → **сразу** (не дожидаясь тика) шлёт
  DialHint на все candidates пира front-first → первый candidate снимает
  rate-limit через dropped-маркер, остальные лимитятся по (peer, addr)
  независимо → гонка, первый validated сокет побеждает.
- Ожидание: если живой транспорт есть — релинк ≈ время handshake (суб-
  секунды–секунды). Если все мертвы — пир уходит в offline через 30с
  (`heard_recently` истекает), редиал продолжается каждые ~10с по всем
  кандидатам.
- Проверено живьём: убийство `adb forward` → дроп; возврат форварда →
  релинк на следующем тике редиала.

### F-11. Beacon-advert не должен ломать доказанный рандеву

- Ситуация: телефон в in-link beacon рекламирует свой wifi-адрес
  `.46:17894`, который на изолированной AP мёртв.
- Правило: `lan_addr_set` (валидация линка) промоутит адрес в начало;
  `lan_addr_hint` (beacon, seed) только **добавляет** альтернативу в
  хвост (cap 4, вытесняет старейшую) и вообще не трогает список, пока
  линк жив. Мёртвая альтернатива не может сместить доказанный адрес.

### F-12. Смена DHCP-адреса пира

- Триггер: у пира сменился ip; старые candidates мертвы.
- Ожидание: beacon с нового адреса даёт хинт → кандидат в хвост →
  редиал-гонка находит его; валидация промоутит в начало. На
  broadcast-глухой сети — через транзитный LanBeacon или ручной dial.

---

## 3. Фичи поверх меша

### F-13. Выборы активной ноды (clipboard routing)

- Правило: `active` = последняя нода с свежим Activity-фреймом;
  при равной активности — freshest wins; телефон не кандидат.
- Ожидание: флип только когда обе машины реально активны — это
  семантика «буфер за последней тронутой машиной», не баг.

### F-14. Clipboard ноут → телефон

- Триггер: копия на активном ноуте.
- Ожидание: клип доезжает до телефона (прямо или транзитом) и
  применяется **только если** отправитель — `activeDevice()`;
  клипы от неактивного ноута отбрасываются на телефоне по дизайну.

### F-15. Clipboard телефон → ноут

- Триггер: share-интент в murmur (Android не даёт фонового чтения
  буфера — только explicit push).
- Ожидание: push применяется на текущем clip-target;
  `last_clip = dir:in, origin:phone`. Anti-echo: применённый клип не
  уезжает обратно.

### F-16. Notifications телефон → ноуты

- Триггер: любая нотификация на телефоне (NotificationBridge listener
  должен быть привязан; после reinstall APK системный биндинг может
  залипнуть — лечится тоглом listener в настройках).
- Ожидание: `org.freedesktop.Notifications.Notify` на шине **каждого**
  ноута — прямым линком и транзитом. Дедуп по id — повторная доставка
  транзитом не дублирует окно.

### F-17. Media: state телефон → ноуты, control ноут → телефон

- State: `playerctl -p murmur` показывает трек/статус телефона на
  всех ноутах (транзит работает).
- Control: pause/play/seek/next/prev с любого ноута доезжают до
  медиасессии телефона; позиция меняется.
- Edge: при старте сервиса `getActiveSessions` может вернуть пусто
  (гонка с биндингом listener) → `rebindIfDark()` на каждом link-up
  перепривязывает контроллер.

### F-18. Presence-таблица и online-семантика

- Ожидание: `murmurctl devices` online = прямой линк **или** валидный
  фрейм за последние 30с. Транзитные члены не «серые». При полной
  потере связи пир становится offline детерминированно через 30с.

---

## 4. Управление семьёй

### F-19. Revoke устройства

- Триггер: `murmurctl revoke <id>`.
- Ожидание: у revoked-устройства ключи мертвы — его линки не валидируются;
  остальные не дозваниваются до него (revoked-гард в dial_loop), его
  фреймы дропаются; `devices --all` показывает revoked. Автоматическая
  ротация family key после revoke (F-20).

### F-20. Ротация ключа

- Триггер: `murmurctl rotate` или автомат после revoke.
- Ожидание: новый family epoch расходится по живым линкам; revoked не
  получает ключ и навсегда слепнет; e2e ротации покрыт тестом.

### F-21. Рестарт демона / ребут машины

- Ожидание: roster, candidates, ключи персистентны (`lan_addrs.json`,
  старый flat-формат `{id: "addr"}` читается обратно-совместимо).
  После старта — redial по всем candidates, релинк без участия юзера.

### F-22. Uninstall

- `murmurctl uninstall` без живого демона: снимает unit, секреты,
  бинари, PATH-маркеры.

---

## 5. Защитные инварианты (не флоу, а «никогда»)

- **I-1.** Self-link невозможен: фреймы с `env.from == self` дропаются на
  адопции; seed на собственный listener не записывается.
- **I-2.** Revoked пир никогда не дозванивается и не валидируется.
- **I-3.** Не-phone пиры: инициирует только меньший device id — ни одна
  пара не держит два линка дольше settle.
- **I-4.** Недоверенные (не trusted) hints — только внутри локальной
  подсети; внешние адреса из beacon не дозваниваются.
- **I-5.** Мёртвый candidate не блокирует живой: коннекты параллельны,
  таймаут 4с, rate-limit по (peer, addr).
- **I-6.** Пока линк жив, hints не переписывают candidates.

---

## 6. Serverful (релей) — отличия флоу

Serverful-семья добавляет релей; всё LAN-выше продолжает работать как
fast path (beacon'ы доезжают и через релей → `LanBeacon` хинтит dialer).

| Флоу | Serverless | Serverful |
|---|---|---|
| Линки | только LAN/Usb транспорты | LAN fast path + relay fallback: конверты идут через релей, даже если прямого пути нет |
| Discovery | UDP-beacon + redial + manual dial | то же + LanBeacon внутри relay-конвертов (cross-LAN discovery) |
| Обрыв всего LAN | offline через 30с | связность сохраняется через релей — online не гаснет |
| Паринг | pair-сокет :17895 (или adb reverse) | весь SAS-хендшейк идёт через релей — LAN не нужен вообще; инвайтер минтит свежий device id на каждый паринг |
| Revoke | `murmurctl revoke <id>` — ревок едет по семейному проводу | `murmurs revoke --id <id>` **на хосте релея** — wire-ревока чужих нет («can only revoke self»); сервер мгновенно рвёт сессию, демон видит `auth rejected: revoked` при реконнекте |
| Мобила+2 ноута | телефон транзитен через ближайший линк | телефон может держать собственную relay-сессию с любой сети |

Детерминированный инвариант serverful: **доставка конверта не зависит
от топологии LAN** — есть хоть один путь (прямой, транзит, релей) —
конверт доезжает, дедуп глушит копии.

---

## 7. Матрица живого прогона (2026-11-28, коммиты 8015577+)

### Serverless (продакшн-меш comp ⇄ laptop ⇄ phone/USB)

| Флоу | Как гоняли | Результат |
|---|---|---|
| F-1 паринг телефона | invite + adb reverse :17895 | OK (SAS match, регистрация) |
| F-3 паринг ноут→ноут | invite + join по LAN | OK |
| F-5 beacon discovery | comp⇄laptop wifi | OK |
| F-6 redial без broadcast | форвард как единственный путь | OK |
| F-7 tie-break | двойные адопции при dial | settle на winner, флапов нет |
| F-8 транзит online+rtt | comp⇄phone через laptop | OK: online + RTT (ICMP-pon­ger semantics) |
| F-9 adb-forward | dial 127.0.0.1:27894 | OK, порт сокета запомнен |
| F-10 фейловер | `adb kill-server` → вернуть forward | OK: дроп мгновенный, instant-redial по всем кандидатам; релинк 5с после возврата транспорта, hands-free |
| F-11 advert не ломает rendezvous | phone beacon .46:17894 | proven 127.0.0.1:27894 в начале, .46 альтернативой |
| F-13 элекция | рестарт comp → обе ноды elect comp | OK, конвергенция за heartbeat |
| F-14 clip laptop→phone | wl-copy на active + гейт | OK + доказательство гейта: клип от неактивного дропнут (`clip from 4a57 active=13f1` без applied), от активного — `applied 13B` |
| F-15 clip phone→laptop | share-intent push | OK (`last_clip=in, origin=phone` на clip-target) |
| F-16 notifications | `cmd notification post` | OK: Notify ×1 на laptop + ×1 на comp (транзит), дедуп не дублирует |
| F-17 media | playerctl state+control | OK: `Paused Day Night` на обоих (транзит), play/pause с comp меняют состояние телефона |
| F-18 online-семантика | transit member | OK |
| F-20 rotate serverless | `murmurctl rotate` на живом мэше | OK: epoch 1 на обоих ноутах, линки не дропнулись, транзит жив |
| F-21 рестарт демона | systemctl restart ×N за сессию | OK, релинк+кандидаты персистентны |
| F-22 uninstall | — | не гоняли (деструктивно, тривиальный путь) |

### Serverful (throwaway-семья через релей `home`/murmur.kolo.ink, LAN fast-path отключён — порт занят → чистый relay-путь)

| Флоу | Как гоняли | Результат |
|---|---|---|
| Релей обновлён | murmurs пересобран на aarch64-хосте из текущего дерева, заменён в /usr/local/bin | active, 101 на WS-handshake, murmurd authed через Cloudflare |
| Паринг через релей | invite на test-node-a + `murmurd join` на свежей test-node-b | OK: SAS-хендшейк целиком через wss, без LAN; инвайтер заминтил свежий id `cebded…` |
| Доставка не зависит от LAN | оба тестовых демона без LAN-порта | OK: взаимная видимость online, RTT 84ms чистым релеем |
| Rotate serverful | `murmurctl rotate` на node-a | OK: epoch 1 доехал до node-b через релей за ~1с |
| Revoke serverful | `murmurs revoke --id` на релее против живой node-b | OK: сессия мгновенно сброшена; реконнект → `auth rejected: revoked` → «re-pair it» |
| Wire-revoke чужого | `murmurctl revoke` на node-a | отказ по дизайну: «can only revoke self» — зафиксировано в §6 |

Закодировано в автотестах (все зелёные на коммите этого абзаца):

- **murmurd 34/34** (`cargo test -p murmurd`): кандидатный порядок/
  promotion/cap F-11, dropped-маркер и мгновенный редиал F-10, per-addr
  rate-limit F-6, tie-break F-7, heard_recently F-8/F-18, адресная
  маршрутизация флуда (pong-инвариант: транзит да, self/@server нет),
  self-guard I-1, анти-echo F-15, ротация e2e F-20, dedupe-матрица
  (twin/out-of-order/restart-gap/bounded), Leaving un-hears instantly.
  Dial-гарды I-2/I-3/I-4/I-5 pinned в `peer_dial_permitted`: revoked
  никогда не диалится, lower-id-initiates для laptops, phones диалятся
  всегда, untrusted-вне-подсети отказ, winner-link-up отказ,
  loser-link-up НЕ блокирует, rate-limit 10с и one-shot bypass по
  dropped-маркеру; `ipv4_in_subnet` bitwise-матрица. Flap-settle: обе
  ноды при худшем interleave сходятся на одном физическом сокете, loser
  churn не эвиктит winner. Паринг: PairHello потребляет invite ровно
  раз (retry игнорируется — F-4 gap pinned), confirm минтит свежий id и
  повторный confirm падает; RegisterDevice+PairData на проводе.
  Clipboard reverse-loop: echo-фрейм легитимно транзитит, но
  `should_apply` по content-hash (не sender) убивает петлю.
- **murmurs 12/12** (`cd server && cargo test`): реестр (auth/revoke/
  persistence/bootstrap-window/rename/family_name) + wire-level на
  живых TCP+WS: точные hello-ошибки (bad token/proto/revoked), pair-conn
  не вытесняет живое устройство, cross-revoke отказ, admin-revoke убивает
  живую сессию, push доезжает членам, pairing-conn взаперти до PairOpen,
  auth-роутинг unicast/broadcast + перезапись `from`.
- **proto 19/19**: golden wire-векторы byte-exact, SAS/MITM, DH-канал,
  crossed-sessions, rekey wrong-recipient, caps, fuzz-light.
- **mobile 20/20** (`./gradlew testDebugUnitTest`): golden-векторы на
  Kotlin-стороне, PresenceTable, Dedupe-матрица (зеркало murmurd `Seen`
  — twin/out-of-order/restart-gap/bounded/per-peer), tie-break mirror-
  symmetry `winnerDirOf` против `winner_dir`.

Не закодировано (живой прогон выше или не гоняли): F-12 DHCP-смена
адреса end-to-end, ICMP RTT точность живьём, F-22 uninstall.
