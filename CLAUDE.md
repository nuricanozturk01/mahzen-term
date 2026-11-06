# CLAUDE.md — MahzenTerm

Rust ile yazılan, modüler, native GPU terminal + çalışma yüzeyi. Ürün vizyonu ve tam mimari: **`PLAN.md`** (kaynak doğruluk).
Bu dosya: kod yazarken uyulacak kurallar.

## Kullanıcı tercihleri
- İletişim Türkçe, kısa ve net. Kod, commit mesajı, kod yorumları İngilizce.
- Commit/push yalnız kullanıcı isterse. `sudo` gerektiren komutları kullanıcı çalıştırır.
- Büyük kararlarda önce PLAN.md'ye bak; çelişki varsa kullanıcıya sor, sessizce sapma.

## Mimari kurallar (ihlal = hata)
1. `core` UI crate'i (iced, wgpu, winit) içermez.
2. `ui-kit` yalnız `core` ve `term`'e bağımlı olur. `modules/*` yalnız `core` ve `ui-kit`'e bağımlı olur. Modüller birbirini ve `app`'i import etmez.
3. Modüller arası iletişim yalnız `core::bus` (Command/Event). Doğrudan çağrı yok.
4. OS'e özgü kod (`cfg(windows)` vb.) yalnız `platform` crate'inde. Başka yerde `cfg(target_os)` kullanma.
5. UI thread'de bloklayan G/Ç, ağ, kilit beklemesi yok. Ağır iş `tokio` veya özel thread.
6. Boşta sürekli yeniden çizim yok: render damage ile tetiklenir, zamanlayıcıyla değil.
7. Yeni özellik → yeni modül crate'i. Çekirdeğe özellik ekleme; yalnız genel mekanizma ekle.
8. Mağaza/tema/keymap/profil = bildirimsel dosya (TOML), kod değil.

## Rust standartları
- Edition 2021+, `rust-version` workspace'te sabit. `#![forbid(unsafe_code)]` varsayılan; `unsafe` yalnız `term`/`platform`'da, `// SAFETY:` yorumuyla ve gerekçeyle.
- Hata: kütüphanede `thiserror`, ikilide `anyhow`. `unwrap()`/`expect()` üretim kodunda yasak (testte serbest; değişmez durum için `expect("neden")`).
- Log: `tracing`. `println!` yok. Sır/token loglama.
- Public API'ye `///` doküman. Modül başına kısa `//!` amaç açıklaması.
- Asenkron: `tokio`. Kanal: `mpsc`/`broadcast`. `Mutex` tutarken `.await` yok.
- Bağımlılık eklerken: bakım durumu, lisans (`cargo-deny`), derleme süresi, ikili boyut kontrol et. Gerekçeyi PR/ADR'ye yaz.
- Biçim: `cargo fmt`. Lint: `cargo clippy --all-targets --all-features -- -D warnings`.

## Performans bütçesi
Soğuk açılış < 150 ms · boşta CPU ~%0 · boşta RAM < 80 MB · tuş→ekran < 12 ms p95.
Render/ayrıştırma yoluna dokunan değişiklik `criterion`/`vtebench` ile ölçülür; gerileme > %10 kabul edilmez.
Sıcak yolda ayırma (allocation) azalt: tamponları yeniden kullan, `Vec::with_capacity`.

## Test
- Her yeni davranış için test. Hata düzeltmesi önce başarısız test (TDD).
- `proptest`: VT ayrıştırma, config birleştirme. `insta`: tema/komut/config anlık görüntüleri.
- Platforma özgü davranış CI matrisinde (ubuntu, macos, windows) doğrulanır.
- "Bitti" demeden önce çalıştır: `cargo fmt --check && cargo clippy ... -D warnings && cargo test --workspace && cargo run -p xtask -- check-deps`. Çıktıyı doğrula, varsayma.

## Yeni modül ekleme kontrol listesi
1. `modules/<ad>/` crate'i; `Cargo.toml`'da yalnız `core`, `ui-kit` bağımlılığı.
2. `Module` trait'ini uygula; manifest'te gerekli yetenekleri (capabilities) bildir.
3. Komutları kayıt defterine yaz (id, başlık, varsayılan kısayol, `when`).
4. Ayarlarını `[modules.<ad>]` şemasıyla kaydet; varsayılanlar gömülü.
5. `app/Cargo.toml`'a Cargo feature olarak ekle; `app` içinde tek satırla kaydet.
6. Birim test + en az bir olay-yolu entegrasyon testi. `docs/modules/<ad>.md` yaz.

## UX kuralları
- Her eylem komut paletinde bulunur ve kısayolu yeniden bağlanabilir.
- Renkler yalnız tema belirteçlerinden gelir; sabit renk kodu yok. Kontrast ≥ 4.5:1.
- Hareket 150–200 ms, "azaltılmış hareket" ayarına saygı gösterir.
- Hata mesajı: ne oldu + ne yapılır. Çökme yerine bozulmuş durum göster.
- Yıkıcı eylem (silme, k8s delete/scale, prod context) onay ister.

## Güvenlik
- Hiçbir şey onaysız indirilip çalıştırılmaz; komut kullanıcıya gösterilir.
- İndirmeler HTTPS + sha256 doğrulamalı. OSC 52 varsayılan "sor". Kaçış dizisinden komut çalıştırma yok.
- Dış girdi (katalog, config, PTY çıktısı, kubeconfig) güvenilmez kabul edilir: doğrula, sınırla.

## Komutlar
```
cargo build --workspace            # derle
cargo run -p app                   # uygulamayı çalıştır
cargo test --workspace             # testler
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo run -p xtask -- check-deps   # bağımlılık kuralı denetimi
cargo bench -p term                # performans
```
(M0 tamamlanana dek bu komutlar henüz çalışmaz.)

## İş akışı
- Büyük iş: spec → plan → uygulama (superpowers). Aşama kapıları PLAN.md §10.
- Mimari karar → `docs/adr/NNNN-baslik.md` (bağlam, karar, sonuçlar).
- Küçük, odaklı değişiklikler; kapsam dışı refactor yok. Kullanıcı istemeden commit yok.
