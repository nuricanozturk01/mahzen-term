# MahzenTerm — Ürün ve Mimari Planı

> Ad: **MahzenTerm** — *A terminal with depth.* (2026-11-06'da TermForge'dan değiştirildi). Durum: **Taslak v0.1 — onay bekliyor.**
> Tarih: 2026-11-06. Dil: Rust. Hedef: Linux, macOS, Windows.

---

## 1. Vizyon

Hızlı, hafif, güzel bir **native GPU terminal**. Terminalin içinde işlerin çoğunu yönetebildiğin bir **çalışma yüzeyi**:
dosya gezgini, markdown/dosya önizleme, IDE (başta hazır araçlarla), Kubernetes, config geçişi, AI kodlama oturumları
(Claude Code, Codex vb.) ve **TUI uygulama mağazası**. Her özellik **modül**dür; çekirdek küçük kalır.

### Referans: atalay-terminal
`atalaykarahan/atalay-terminal` genel terminal değil; AI oturumları için macOS HUD'u (Swift/AppKit + SwiftTerm, three.js
orbit haritası, `screen` ile kalıcı oturumlar). Windows portu ayrı kod tabanı, Linux yok, sosyal kısım kapalı sunucuda.
**Alınan fikirler:** oturum başına kalıcı pencere/pane, canlı oturum durumu, tema çeşitliliği, Source Control paneli.
**Alınmayan:** platform başına ayrı kod, harici `screen` bağımlılığı, kapalı backend, gamification.

### Başarı kriterleri
| Metrik | Hedef |
|---|---|
| Soğuk açılış | < 150 ms (pencere görünür) |
| Boşta CPU | ~%0 (sürekli çizim yok) |
| Boşta RAM (1 sekme) | < 30 MB (ölçülen: 18 MB, ADR-0002) |
| Girdi gecikmesi (tuş → ekran) | < 12 ms p95 |
| Çıktı akışı (`cat` büyük dosya) | ≥ 200 MB/s ayrıştırma |
| Platform | Linux (X11+Wayland), macOS 13+, Windows 10 1809+ (ConPTY) |

### Hedef dışı (şimdilik)
Bulut senkron, hesap sistemi, sosyal özellikler, mobil, uzak oturum sunucusu. Kendi editör motoru (M7'ye kadar hazır araçlar).

---

## 2. İlkeler

1. **Çekirdek küçük, her şey modül.** Yeni özellik = yeni crate, çekirdeğe dokunmadan.
2. **UI'dan bağımsız `core`.** İleride TUI/headless ön yüz eklemek mümkün olmalı.
3. **Mesajla konuş.** Modüller birbirini çağırmaz; Command/Event yolu kullanılır.
4. **Boşta sıfır iş.** Zamanlayıcıyla sürekli çizim yok; damage tabanlı render.
5. **Önce doğruluk, sonra hız, sonra süs.** VT uyumluluğu ve kararlılık testlerle kanıtlanır.
6. **Veriyle genişle.** TUI mağazası, tema, keymap, k8s profilleri kod değil **bildirimsel dosya**dır.
7. **Her platform birinci sınıf.** Platform farkı yalnızca `platform` crate'inde yaşar.
8. **Güvenli varsayılanlar.** Hiçbir şey kullanıcı onayı olmadan indirilip çalıştırılmaz.

---

## 3. Mimari

### 3.1 Workspace yapısı
```
mahzenterm/
├─ crates/
│  ├─ core/        # olay yolu, workspace modeli, config, modül kaydı, komut sistemi (UI bağımsız)
│  ├─ term/        # PTY, VT durumu (alacritty_terminal), hücre render, seçim, arama, resim protokolleri
│  ├─ ui-kit/      # Iced widget'ları, tema motoru, ikonlar, animasyon, ortak bileşenler
│  ├─ app/         # Iced kabuğu: pencere, sekme, pane ağacı, komut paleti, ayarlar
│  └─ platform/    # OS soyutlaması: PTY/ConPTY, shell tespiti, dizinler, pano, bildirim, güncelleme
├─ modules/
│  ├─ explorer/        # dosya/klasör gezgini
│  ├─ viewer/          # markdown, resim, cat/bat tarzı önizleme
│  ├─ tui-store/       # TUI mağazası (katalog, kurulum, başlatma)
│  ├─ editor-bridge/   # Helix/Neovim vb. entegrasyonu (IDE v0)
│  ├─ ai-sessions/     # Claude Code / Codex / Gemini oturum paneli
│  ├─ kubernetes/      # kubeconfig, context, pod/log/exec
│  └─ config-switcher/ # profil/config geçişi (kube, git, env, ssh…)
├─ catalog/            # TUI mağaza manifestleri (TOML + ss), koddan ayrı sürümlenir
├─ themes/             # yerleşik temalar (TOML)
├─ docs/               # ADR'ler, modül yazma rehberi
├─ xtask/              # build/paketleme/denetim yardımcıları
└─ PLAN.md, CLAUDE.md
```

### 3.2 Bağımlılık kuralları (CI'da zorlanır)
```
modules/*  →  core, ui-kit            (birbirine ve app'e ASLA)
app        →  core, term, ui-kit, platform, modules/* (feature ile)
term       →  core, platform
ui-kit     →  core, term           (terminal görünümü Snapshot tüketir)
core       →  (UI crate'i yok, iced yok)
platform   →  (yalnız OS crate'leri)
```
Denetim: `cargo-deny` + `xtask check-deps` (workspace grafını kurala göre doğrular).

### 3.3 Çekirdek bileşenleri

**Olay yolu (`core::bus`)**
- `Command`: hedefli istek (`OpenTab`, `OpenFile{path}`, `RunInPane{cmd}`, `ShowPanel{id}`). Tek alıcı: çekirdek/ilgili modül.
- `Event`: yayın (`PaneSpawned`, `PaneExited`, `CwdChanged`, `ConfigReloaded`, `ThemeChanged`, `FocusChanged`). Çok alıcı.
- Taşıyıcı: `tokio::sync::{mpsc, broadcast}`. Tipli zarf, sürümlü şema (modül uyumu için).
- Modüller olayı abone olur; UI güncellemesi Iced `Subscription` ile köprülenir.

**Workspace modeli (`core::workspace`)**
`Window → Tab → PaneTree(split: H/V, oran) → Pane{kind: Terminal | ModulePanel}`. Serde ile serileştirilir; kapanışta
`state.toml/json` yazılır, açılışta geri yüklenir (cwd, bölünme, sekme adı; çalışan süreç yeniden başlatılmaz, kullanıcıya sorulur).

**Komut sistemi (`core::command`)**
Her eylem kayıtlı bir komuttur (`id`, başlık, varsayılan kısayol, `when` bağlamı). Komut paleti, kısayollar, menü ve modüller
aynı kayıt defterini kullanır. Böylece her şey aranabilir ve yeniden bağlanabilir.

**Config (`core::config`)**
- Katmanlar: gömülü varsayılan < kullanıcı (`~/.config/mahzenterm/config.toml`) < proje (`.mahzenterm/config.toml`).
- Şema `serde` + doğrulama; hatalı config uygulamayı çökertmez, UI'da satır numaralı hata gösterir, son geçerli değer kalır.
- `notify` ile canlı yenileme; `schema_version` + otomatik göç.
- Modüller kendi ayar bölümünü `[modules.<ad>]` altında kaydeder (şema ile).

**Modül sözleşmesi (`core::module`)**
```rust
pub trait Module: Send + 'static {
    fn manifest(&self) -> ModuleManifest;                 // id, ad, sürüm, gerekli yetenekler
    fn register(&mut self, cx: &mut RegisterCtx);         // komut, panel, ayar, kısayol, durum çubuğu öğesi
    fn on_event(&mut self, ev: &Event, cx: &mut Ctx);     // olaya tepki
    fn on_command(&mut self, cmd: Command, cx: &mut Ctx); // kendi komutu
    fn panel(&self, id: PanelId) -> Option<PanelView>;    // görünüm (ui-kit tipleri)
    fn shutdown(&mut self);
}
```
- **Yetenekler (capabilities):** `fs.read`, `fs.write`, `net`, `process.spawn`, `pty`, `clipboard`. Manifest ister, çekirdek kaydeder;
  şimdilik güven tabanlı (in-tree), WASM/harici süreç eklentilerinde zorlayıcı hale gelir.
- **Yaşam döngüsü:** keşfet → kaydet → (tembel) başlat → durdur. Panel ilk gösterimde başlatılır.
- **Hata yalıtımı:** modül paniği `catch_unwind` ile yakalanır, modül "çöktü" durumuna alınır, uygulama ayakta kalır.
- **Üçüncü taraf (M8+):** WASM (wasmtime + WIT) veya JSON-RPC harici süreç. Trait'in verisi serileştirilebilir tutulur.

### 3.4 Eşzamanlılık modeli
- **UI thread:** Iced. Yalnız çizim ve mesaj dağıtımı.
- **PTY thread'i (pane başına):** okur, `alacritty_terminal` ayrıştırıcısına besler, damage bilgisi biriktirir, UI'ya birleştirilmiş (coalesced) `Redraw` mesajı yollar (en fazla ekran tazeleme hızında).
- **tokio runtime:** ağ (k8s API, katalog), dosya izleme, config yenileme, git.
- Kural: UI thread'de G/Ç ve kilit beklemesi yok.

---

## 4. Terminal Motoru (`term`)

| Konu | Karar |
|---|---|
| VT durumu/ayrıştırma | `alacritty_terminal` (olgun, hızlı, VT uyumlu) |
| PTY | `portable-pty` (Unix PTY + Windows ConPTY) |
| Metin şekillendirme | `cosmic-text` (fallback fontlar, emoji, bidi, ligature), glyph atlas |
| Render | Iced **tiny-skia (CPU)** varsayılan, `gpu` feature ile wgpu opt-in (ADR-0002); hücre ızgarası geometri önbelleği, yalnız yeni çıktıda yeniden çiz; kutu/blok karakterleri yerleşik dikdörtgenlerle |
| Font | Varsayılan gömülü Nerd Font uyumlu mono; sistem fontu seçimi; ligature açık/kapalı |
| Renk | 16/256/truecolor, tema paleti, minimum kontrast koruması |
| Seçim/pano | Satır/blok/kelime seçimi, bracketed paste, OSC 52 (onaylı), URL algılama (Ctrl+tık) |
| Arama | Scrollback içinde regex arama, vurgulama |
| Scrollback | Sınırlı halka tampon (varsayılan 10k satır), disk taşması yok |
| Resim protokolleri | Kitty graphics (öncelik), Sixel, iTerm2 inline — M3'te |
| Shell entegrasyonu | OSC 133 (prompt/komut sınırları), OSC 7 (cwd) → komut bloğu, "son komutu kopyala", cwd takibi |
| Hyperlink | OSC 8 |
| Giriş | Kitty keyboard protocol, modifyOtherKeys; IME desteği |
| Platform | Windows ConPTY farkları (`platform`), WSL ve PowerShell/cmd/pwsh/bash/zsh/fish algılama |

**Doğrulama:** `vttest`, esctest alt kümesi, `vtebench` ve `criterion` ile throughput, kare süresi izleme (`--perf-hud`).

---

## 5. Modüller (ayrıntı)

### 5.1 explorer
Ağaç ve liste görünümü, sanal liste (100k dosyada akıcı), `.gitignore` farkındalığı, git durum rozetleri,
bulanık arama (`nucleo`), dosya işlemleri (yeniden adlandır, taşı, sil → çöp kutusu), "bu klasörde terminal aç",
sürükle-bırak yol yapıştırma. Dosya izleme `notify`.

> Explorer: tıklanabilir yenileme düğmesi (↻) + `F5`/`r` eklendi (2026-11-06).

### 5.2 viewer
`mdcat` benzeri markdown (başlık, tablo, kod bloğu sözdizimi `syntect`/`tree-sitter-highlight`, görsel), kod, JSON/YAML/TOML,
resim, CSV tablo. Terminalde `tf cat <dosya>` ve gezginde önizleme paneli. Kaynak ↔ render geçişi.

### 5.3 tui-store
- **Katalog = veri:** `catalog/<ad>/app.toml` + `screenshot.png`.
```toml
id = "yazi"
name = "Yazi"
summary = "Hızlı dosya yöneticisi"
categories = ["files"]
screenshots = ["shot1.png"]
command = "yazi"
[install]
cargo = "yazi-fm"
brew = "yazi"
winget = "sxyazi.yazi"
apt = "yazi"
binary = { url = "...", sha256 = "..." }
[platforms]
supported = ["linux", "macos", "windows"]
```
- Arayüz: ızgara/kart, kategori, arama, ss galerisi, "Kur" → "Aç" (yeni sekme/pane).
- Kurulum sırası: platforma uygun paket yöneticisi → `cargo binstall` → doğrulanmış ikili (sha256). **Her kurulum onay ister**, komut gösterilir.
- Katalog kaynakları: gömülü + kullanıcı eklediği git/URL kaynakları. Güncelleme imzalı/özetli.
- Kullanıcı kendi TUI'sini ekleyebilir (form → manifest üretir).

### 5.4 editor-bridge (IDE v0)
Helix/Neovim/Micro'yu pane'de aç, `OpenFile{path,line}` komutuyla dosyadan/gezginden/hata çıktısından yönlendir.
Editör algılama ve kurulum önerisi (tui-store ile entegre). Dosya:satır tıklama → editörde aç.
**Sonraki (M7):** yerleşik hafif editör (`ropey` + `tree-sitter`, LSP istemcisi `lsp-types`), git paneli, proje araması.

### 5.5 ai-sessions
Claude Code, Codex, Gemini CLI vb. oturumları **pane içinde normal PTY süreci** olarak çalışır; modül yan panelde
oturum listesi, durum ve maliyet gösterir. Tasarım, `atalay-terminal` kaynağının okunmasına dayanır
(araştırma: `docs/research/atalay-terminal.md`).

**Durum kaynağı (Claude Code):** süreç bilgisi değil, Claude'un kendi yerel dosyaları (best-effort, sürüm değişirse adaptör kırılır):
- `~/.claude/sessions/<pid>.json` (veya `CLAUDE_CONFIG_DIR`): canlı kayıt. Alanlar: `sessionId`, `pid`, `status`
  (`busy`/`idle`/`waiting`/`shell`), `waitingFor`, `kind`, `updatedAt`, `statusUpdatedAt`, `cwd`, `name`.
  Yalnız `kind == interactive`, `spare != true`, `hostSessionId` boş olanlar kullanıcının sohbetidir.
- `~/.claude/projects/<kodlanmış-cwd>/<sessionId>.jsonl`: transcript. Satır tipleri: `assistant` (token `usage`, `tool_use`),
  `user` (prompt, `tool_result`), `system`, `ai-title`, `last-prompt`. Alt ajanlar `isSidechain`/`agentId` taşır.
  `cwd` kodlaması: ASCII alfanümerik olmayan her karakter `-`.
- **Artımlı okuma:** dosya başına byte offset + artık satır tamponu; dosya küçülürse sıfırla; büyük dosyada
  gün başına göre ikili arama ile atla. Dosya izleme macOS'ta FSEvents; bizde `notify` (inotify/FSEvents/ReadDirectoryChangesW).
- **Tekrarlar:** compact/resume yeni `sessionId` açar ve kayıtları tekrarlar; `uuid` ile tekilleştir, ilk sahibi "devralınan" işaretle.
- **Fiyat:** model öneki → ($/M token: input, output, cache-read, cache-write, bağlam penceresi) tablosu; tablo koda gömülmez,
  `catalog/pricing.toml` olarak güncellenebilir. Maliyet "liste fiyatı tahmini" diye etiketlenir.

**Diğer araçlar (Codex, Gemini, Aider, OpenCode…):** süreç adı veya yorumlayıcı betik adıyla algıla
(`codex`, `gemini`, `opencode`, `aider`, `cursor-agent`, `agy` …; `node/bun/python` betik adı). Durum: CPU kullanımı ve çocuk süreçler
(10 sn sessizlik = boşta). Codex için `~/.codex/sessions/YYYY/MM/DD/*.jsonl` (ilk satırdaki `payload.cwd` ile eşleştir),
Antigravity için kendi SQLite/log dosyaları. Bunlar kırılgandır: her biri `adapters/<araç>.rs`, ortak `AiAdapter` trait'i:
`detect(&ProcInfo) -> Option<Tool>`, `poll(&mut State) -> Vec<SessionEvent>`. Sağlayıcı formatı değişirse yalnız o adaptör "bilinmiyor" durumuna düşer.

**Süreç bilgisi:** `platform::proc` (pid → ad, argümanlar, ppid, tty/pty, CPU süresi, RSS). `sysinfo` crate'i; ek olarak
`/proc` (Linux), `libproc` (macOS), Toolhelp (Windows) farkları yalnız `platform`'da.

**Kalıcı oturum (M6):** atalay `screen` kullanır (macOS), Windows'ta ConPTY'yi tutan ayrı bir "session host" süreci.
Bizde tek çözüm: `mahzenterm-host` arka plan süreci PTY'leri sahiplenir; uygulama IPC (Unix socket / named pipe) ile bağlanır.
Çıktı geçmişini host `alacritty_terminal` durumu olarak tutar, yeniden bağlanınca ekranı geri gönderir.
Host yoksa uygulama içinde çalışır ve kapanınca biter (kullanıcıya uyarı). Harici `screen`/`tmux` bağımlılığı yok.

**Arayüz:** oturum kartı (ad, dal, durum rozeti, model, bağlam doluluğu çubuğu, maliyet, çalışan araç), "bekliyor" bildirimi
(izin istiyor), tıkla → ilgili pane'e odaklan. Orbit haritası gibi süs yerine okunabilir liste + küçük sparkline (M6'da gözden geçirilir).

**Bilerek yapılmayanlar:** başka araçların ayar dosyalarına kanca (hook) yazmak (atalay'ın TokenSavers'ı rtk/caveman'ı
kullanıcının Claude/Codex ayarlarına kurar; biz kullanıcı onayı olmadan başka aracın yapılandırmasını değiştirmeyiz),
sunucuya telemetri, sosyal özellikler.

### 5.8 media (görsel görüntüleyici + video oynatıcı)
Terminal içinde her biçimi açan görüntüleyici ve oynatıcı; kendi pane türüdür (`PaneKind::Media`), PTY gerektirmez.
- **Görsel:** `image` crate'i (PNG/JPEG/GIF/WebP/BMP/TIFF/ICO/AVIF…), SVG (`resvg`), animasyonlu GIF/WebP; yakınlaştır, kaydır, döndür, klasörde önceki/sonraki.
- **Video/ses:** kodek yükünü kendimiz yazmayız: `libmpv` (veya `ffmpeg` süreci + kare akışı) ile çözme; arayüz bizde (oynat/duraklat, ara, ses, hız, altyazı). Hedef: mp4/mkv/webm/mov/avi/mp3/flac…
- **Tarayıcıdan açma:** `mahzenterm open <url|dosya>` ve OSC 8 / URL tıklama, medya URL'lerini (doğrudan dosya, YouTube vb. `yt-dlp` varsa) tarayıcı yerine pane'de oynatmayı önerir. Her indirme/komut kullanıcıya gösterilir, onay ister (§7).
- **Taşınabilirlik (yapıldı, panel yüzeyleri için):** uygulama artık çok pencereli (`iced::daemon`); token ve oturum panelleri kart, sekme veya ayrı pencere olarak açılır (`docs/placement.md`). Terminal pane'ini pencereye koparmak ve medya pane'i için aynısı hâlâ yapılacak.
- **Taşınabilirlik (medya):** medya pane'i bir sekmeden diğerine sürüklenir, **ayrı pencereye** koparılır (`Detach to Window`) ve 2./3. monitöre konur; pencereler arası geri takılır. Oynatma durumu (konum, ses) taşınırken kesilmez. Bu, çok pencereli çalışma alanı (`Window` listesi) gerektirir: çekirdekte genel mekanizma olarak eklenir, medya yalnız onu kullanır.
- **Belgeler (yapıldı, ilk dilim):** `tf cat`/`tf view` Word, PowerPoint, OpenDocument metin ve elektronik tabloları okur (saf Rust, sınırlı); PDF poppler ile (`tf pdf` sayfa sayfa görüntü, `tf cat` metin). Düzenleme yok. Sandbox'lı WASM okuyucular ileride aynı arayüzü alabilir (bkz. M8 tasarımı, eklenti fikirleri).
- **Terminal içi:** `tf img <dosya>` / `tf play <dosya>` mevcut pane'de ya da yeni pane'de açar; Kitty/Sixel (M3) ile satır içi gösterim ayrı.
- `[modules.media]`: varsayılan oynatıcı arka ucu, donanım çözme, ses cihazı.

### 5.10 Kısayol düzenleme — planlandı
Her komutun kısayolu hem **config dosyasından** (`[[keys]]`, zaten var) hem **uygulama içinden** değiştirilebilir olmalı.
- Uygulama içi: palette "Keyboard Shortcuts" → komut listesi (arama), Enter = "yeni tuşa bas", Esc iptal, Backspace = kısayolu kaldır.
  Çakışma varsa uyarı + hangi komutla çakıştığı gösterilir; kayıt `config.toml` içindeki `[[keys]]` bölümüne yazılır (yorumlar korunur),
  canlı yeniden yüklenir. "Varsayılana dön" komutu da bulunur.
- Yeni her komut otomatik olarak listede çıkar (kayıt defterinden); modül komutları da dahil.

### 5.9 browser (terminal içi tarayıcı) — planlandı
Kullanıcı isteği: terminalin içinde çalışan bir tarayıcı. Panel yüzeyi olarak (kart/sekme/pencere) açılır, `Ctrl+Shift+P` → "Browser: Open URL".
- **Aşama 1 (metin tarayıcı):** `reqwest`(rustls)+`html5ever`/`scraper` ile sayfayı alıp okunabilir metne çevirir (başlık, bağlantı numaraları, formlar yok);
  bağlantılar numara/Tab ile gezilir, geri/ileri, yer imi. Görseller Kitty/iTerm2 grafikleriyle (`tf img` yolu). JS çalışmaz.
- **Aşama 2 (tam sayfa):** harici bir motoru ekran görüntüsü akışıyla gösterme (ör. kullanıcının `chromium --headless` / `lightpanda`'sı);
  fare/klavye olayları motora iletilir. Varsayılan KAPALI, kullanıcı açıkça etkinleştirir ve komut gösterilir.
- Güvenlik: yalnız https/http; indirme yok (indirme onaylı, sha256 ayrı akış); çerez/oturum varsayılan kalıcı değil; `file://` ve yerel ağ adresleri
  varsayılan engelli (SSRF), sayfa içeriği güvenilmez girdi (boyut/zaman sınırı), kaçış dizisi temizlenir.
- Mevcut `tf play` (yt-dlp) ve görüntüleyiciyle birlikte çalışır: görsel/video/PDF bağlantıları ilgili görüntüleyicide açılır.

### 5.6 kubernetes
`kubeconfig` okuma, context/namespace değiştirici (durum çubuğunda), pod/deployment listesi (`kube-rs`), log akışı,
`exec` bir pane'de, port-forward yönetimi, kaynak YAML görüntüleme/düzenleme (editör köprüsü), canlı durum rozetleri.
Yalnız okuma varsayılan; yıkıcı eylemler (delete/scale) onay ister. Prod context'te kırmızı uyarı bandı.

> **Karar (2026-11-06, ADR 0003):** ilk dilim `kube-rs` yerine kullanıcının `kubectl`'ini sürer (salt okunur JSON sorguları;
> log/exec/describe pane'de). Gerekçe: kimlik eklentileri hazır çalışır, ağır istemci bağımlılığı yok, kimlik bilgisi okunmaz.
> Canlı izleme/port-forward yönetimi gelince `kube-rs` arka ucu `kubectl.rs`/`parse.rs`'in yerine geçebilir. İtiraz edersen söyle.

### 5.7 config-switcher
Adlandırılmış profil: ortam değişkenleri, kube context, git kimliği, SSH config, AWS/GCP profili, tema.
Hızlı geçiş (komut paleti, durum çubuğu), proje girişinde otomatik uygula (`.mahzenterm/profile`).
Değişiklikler çalışan pane'lere yeni süreçlerde uygulanır; mevcut shell'e `export` önerisi sunulur.

---

## 6. UX ve Görsel Tasarım

**İlkeler:** klavye öncelikli, fareyle de kusursuz; her şey komut paletinden bulunur; az ama net görsel dil; hareket anlam taşır (150–200 ms, azaltılabilir).

- **Kabuk:** sol dar aktivite çubuğu (modül simgeleri) · kenar paneli (gezgin vb.) · merkez pane ağacı · alt/üst durum çubuğu (cwd, git dalı, k8s context, profil).
- **Sekmeler:** sürükle-sırala, renk/ikon, oturum adı otomatik (cwd + süreç), bölme (`Ctrl+Shift+D/E`), yakınlaştır (zoom pane).
- **Komut paleti** (`Ctrl+Shift+P`): bulanık arama, son kullanılanlar, bağlama göre öncelik.
- **Tema motoru:** TOML tema (palet + UI belirteçleri), ışık/koyu otomatik, canlı önizleme, blur/saydamlık (platform destekliyse).
  Yerleşik: MahzenTerm Dark/Light, Catppuccin, Tokyo Night, Dracula, Nord, Gruvbox, Solarized.
- **Boş durum & ilk açılış:** hoş geldin sayfası: shell seçimi, font/tema seçimi, mağazadan öner.
- **Grafikler:** canvas tabanlı küçük grafikler (CPU/mem pod metrikleri, AI token kullanımı), erişilebilir renk paleti, ışık/koyu uyumlu.
- **Erişilebilirlik:** kontrast ≥ 4.5:1, yüksek kontrast teması, ölçeklenebilir UI (HiDPI), azaltılmış hareket seçeneği, klavye ile tüm gezinti.
- **Dil:** i18n altyapısı (`fluent`), başlangıçta TR + EN.

---

## 7. Güvenlik

- Mağaza: indirme yalnız **HTTPS + sha256**, kurulumdan önce komut gösterilir ve onay alınır; katalog güncellemeleri doğrulanır.
- OSC 52 pano yazma/okuma ayarlı ve varsayılan "sor"; kaçış dizisi ile komut çalıştırma yok.
- Yapıştırmada bracketed paste + çok satır uyarısı.
- Sırlar (kubeconfig token, API anahtarı) log'a yazılmaz; `tracing` alanlarında maskeleme.
- Bağımlılık denetimi: `cargo-audit`, `cargo-deny` CI'da. Güncellemeler imzalı.
- Eklenti yetenek modeli (§3.3) üçüncü taraf eklentide zorunlu.

---

## 8. Kalite, Test ve CI

| Katman | Yöntem |
|---|---|
| Birim | `cargo test`, `proptest` (VT dizisi, config birleştirme) |
| Anlık görüntü | `insta` (config, komut kaydı, tema çözümü) |
| Terminal uyumluluk | vttest/esctest alt kümesi, kayıtlı oturum tekrar oynatma |
| Performans | `criterion` + `vtebench`; CI'da gerileme eşiği (örn. +%10 → kırmızı) |
| UI | Iced headless render karşılaştırma (anahtar ekranlar) |
| Platform | GitHub Actions matris: ubuntu, macos (arm+x86), windows |
| Statik | `clippy -D warnings`, `rustfmt`, `cargo-deny`, `cargo-audit`, `xtask check-deps` |
| Kapsam | Kritik crate'lerde hedef ≥ %70 (`cargo-llvm-cov`) |

---

## 9. Paketleme ve Dağıtım

`cargo-dist` + platform paketleri: Windows (MSI + winget + scoop), macOS (imzalı/notarize `.dmg` + Homebrew cask), Linux (AppImage, `.deb`, `.rpm`, Flatpak, AUR).
Otomatik güncelleme: kanal (stable/nightly), imzalı manifest, kullanıcı onayı. Telemetri: **yok** (ileride yalnız opt-in).

---

## 10. Yol Haritası

| Aşama | Çıktı | Kabul ölçütü |
|---|---|---|
| **M0 İskele** | workspace, CI matrisi, `check-deps`, lisans, ADR şablonu, boş Iced penceresi | 3 OS'te CI yeşil, pencere açılır |
| **M1 Terminal çekirdeği** | PTY + `alacritty_terminal` + GPU hücre render, klavye/fare, seçim, scrollback, tek pane | `vim`, `htop`, `ls --color` doğru; perf hedefleri ölçülür |
| **M2 Kabuk** | sekme, bölme, komut paleti, config + canlı yenileme, tema motoru, durum çubuğu, oturum geri yükleme | Günlük kullanım için yeterli terminal |
| **M3 Zengin terminal** | OSC 133/7/8/52, Kitty graphics + Sixel, arama, URL, ligature, IME | `yazi`/`chafa` resimleri, shell entegrasyonu çalışır |
| **M4 explorer + viewer** | dosya gezgini, markdown/cat önizleme, `tf` CLI | `tf cat README.md` güzel render |
| **M5 tui-store + editor-bridge** | katalog, kurulum, ss galerisi, Helix/Neovim köprüsü, dosya:satır yönlendirme | Mağazadan `yazi` kur-aç; hata satırından editör |
| **M5b token izleyici** (öne alındı) | Claude Code/Codex token kartı, ekranda sürüklenen yüzen panel | Kart istenen yere konur, konum kalır |
| **M6 ai-sessions** | oturum paneli, adaptörler, bildirim, (opsiyonel) oturum arka plan servisi | Claude Code + Codex yan yana yönetilir |
| **M7 kubernetes + config-switcher** | k8s paneli, log/exec, profil geçişi | Context değiştir, pod log izle, profil uygula |
| **M8 IDE v1 + eklenti SDK** | yerleşik editör (ropey+tree-sitter+LSP), git paneli, WASM eklenti | Üçüncü taraf modül örneği çalışır |
| **M8b media** | görsel görüntüleyici, video/ses oynatıcı, tarayıcıdan açma, sekme/pencere/monitör arası taşıma | Tarayıcı bağlantısı pane'de oynar; pane ikinci monitöre koparılır |
| **M9 Cilalama** | erişilebilirlik, i18n, paketleme, güncelleyici, dokümantasyon, v1.0 | Tüm platform paketleri yayında |

Her aşama: kendi spec → plan → uygulama döngüsü (alt-proje). Bu belge üst düzey çerçevedir; M0'dan itibaren aşama başına ayrıntılı plan yazılır.

---

## 11. Riskler

| Risk | Etki | Önlem |
|---|---|---|
| Iced'de yüksek performanslı özel terminal widget'ı | Yüksek | M1'de erken prototip + ölçüm; gerekirse `wgpu` render'ı Iced dışında özel pencere katmanına taşı (ui-kit arkasında soyutlanmış) |
| Windows ConPTY tuhaflıkları | Orta | M1'den itibaren Windows CI + gerçek makinede test |
| AI sağlayıcı log formatları değişir | Orta | İzole adaptörler, "best-effort", sürüm tespiti |
| Kitty/Sixel render karmaşası | Orta | M3'e ertele, önce metin doğruluğu |
| Kapsam şişmesi | Yüksek | Aşama kapıları; modül dışı hiçbir şey çekirdeğe girmez |
| Linux Wayland/X11, IME | Orta | winit ile erken test, IME'yi M3'te önceliklendir |
| Tek geliştirici yükü | Orta | Katı modül sınırları, otomasyon, ADR ile karar kaydı |

---

## 12. Kararlar (varsayılan; kullanıcı itiraz edebilir)

1. Ad: **MahzenTerm**, slogan *A terminal with depth*. Lisans: **MIT OR Apache-2.0**.
2. AI arka plan servisi: **M6'da isteğe bağlı**; o zamana dek yalnız uygulama açıkken.
3. Windows varsayılan shell: **PowerShell 7 (`pwsh`), yoksa Windows PowerShell, yoksa cmd**; WSL ayrı profil.
4. Platform sırası: geliştirme **Linux**, CI'da **3 platform** birden.

---

## 13. Sözlük
**Pane:** bölünebilir tek alan. **Panel:** modülün sunduğu kenar/pane görünümü. **Damage:** değişen hücre bölgesi.
**Command/Event:** istek/yayın mesajı. **Capability:** modülün istediği yetki. **Katalog:** TUI mağaza manifestleri.
