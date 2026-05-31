# izole 🚀

`izole`, Linux sistemlerinde sistem dosyalarını ve kütüphanelerini kirletmeden bağımlılık izolasyonu sağlayan, uygulamaları ana makinenin izinleriyle şeffaf şekilde çalıştıran Rust tabanlı bir **Sandbox & Paket Yönetim Katmanıdır**. 

Ayrıca, arka planda çalışan **kendi kendini iyileştirme (Self-Healing)** mimarisi ve yerel **Ollama Yapay Zeka entegrasyonu** sayesinde çalışma hatalarını otomatik analiz eder, çözüm sunar ve onayınızla problemi kendi kendine çözer.

---

## ✨ Özellikler

* 🔒 **Güvenli ve Şeffaf Sandbox:** `bubblewrap` (`bwrap`) altyapısını kullanarak `/usr`, `/etc` ve `/opt` dizinlerini salt-okunur bağlar. Ancak `/home`, `/dev`, `/tmp` gibi kullanıcı dizinlerini yazma izniyle şeffaf şekilde ana makineye bağlar.
* 📦 **İzole Paket Kurulumu:** Sistemdeki paket yöneticisini (**DNF**, **APT** veya **Pacman**) otomatik tespit ederek bağımlılıkları sadece ilgili izole ortamın (`~/.local/share/izole/envs/<ortam>`) içerisine kurar. Sistem kütüphaneleriniz temiz kalır.
* 🎮 **Tam Donanım ve Grafik Desteği:** Ekran sunucuları (X11/Wayland), ses sunucuları (PulseAudio/PipeWire) ve GPU hızlandırma (NVIDIA/AMD) izole uygulamalara şeffaf bir şekilde aktarılır.
* 🛠 **Özel Binary ve Alias Kayıt:** Dışarıdan indirdiğiniz çalıştırılabilir dosyaları ortama kaydedebilir (`register`) ve terminalde doğrudan ismiyle çalıştırabilmek için shims/takma adlar (`alias`) atayabilirsiniz.
* ⚙️ **Otomatik NVIDIA Hizalama:** Sistem güncellemeleri sonrası oluşan çekirdek-sürücü sürüm uyuşmazlığını (`version mismatch`) algılar ve ortam içindeki grafik kütüphanelerini otomatik günceller.
* 🤖 **Ollama AI Teşhisi (Self-Healing):** Uygulama çöktüğünde oluşan `stderr` çıktılarını yerel Ollama modelinize (`minimax-m2.7:cloud`, `llama3` vb.) gönderir. Yapay zeka hatayı Türkçe açıklar, gerekli paketi veya komutu tespit eder ve tek tıkla onayınızla sorunu otomatik gidererek uygulamayı yeniden başlatır.

---

## 🛠 Kurulum

### Gereksinimler
* Rust ve Cargo (Derlemek için)
* Bubblewrap (`bwrap`)
* Sistem Paket Yöneticisi (DNF, APT veya Pacman)
* Ollama (Yapay zeka desteği için isteğe bağlı)

### Derleme ve Yükleme
Projeyi yerel olarak klonlayın ve derleyin:
```bash
git clone https://github.com/c0n419/izole.git
cd izole
cargo build --release
```

Derlenen binary dosyasını kullanıcı yolunuza kopyalayın:
```bash
cp target/release/izole ~/.local/bin/
```
`~/.local/bin` yolunun `PATH` değişkeninizde tanımlı olduğundan emin olun.

---

## 💻 Kullanım Kılavuzu

### 1. Ortam Oluşturma ve Paket Kurma (`install`)
Belirtilen izole ortama RPM paketlerini kurar. Ortam yoksa otomatik oluşturulur.
```bash
izole install btop
```
*Masaüstü kısayolları (`.desktop` dosyaları) otomatik olarak `~/.local/share/applications` altına aktarılır ve uygulama menünüze eklenir.*

### 2. Uygulama Çalıştırma (`run`)
```bash
izole run btop btop
```

### 3. İnteraktif Kabuk Girişi (`enter`)
Ortamın terminaline girerek izole ortam içinde komutlar çalıştırmanızı sağlar:
```bash
izole enter btop
```

### 4. Bilgi Raporlama (`info`)
Ortamın disk boyutunu, kurulu paketleri ve oluşturulmuş masaüstü kısayollarını gösterir:
```bash
izole info btop
```

### 5. Özel Dosya Ekleme & Alias Oluşturma
Harici bir çalıştırılabilir programı ortama entegre edip doğrudan terminal kısayolu oluşturmak için:
```bash
izole register myenv /path/to/my_program
izole alias myenv my_program my-cmd
```
Artık terminale sadece `my-cmd` yazarak çalıştırabilirsiniz.

### 6. Temizlik (`delete`)
Oluşturulmuş tüm ortam dosyalarını, masaüstü kısayollarını ve shims/takma adlarını sistemden kalıcı olarak temizler:
```bash
izole delete btop
```

---

## 🤖 Kendi Kendini İyileştirme (AI Self-Healing) Nasıl Çalışır?

Uygulamanız izole ortamda çalışırken bir eksiklik veya hata nedeniyle çöktüğünde `izole` otomatik olarak devreye girer:

1. **Hata Yakalama:** Çökme hatası (`stderr`) okunur.
2. **AI Analizi:** Ollama API'si üzerinden yerel yapay zeka modeline hata çıktısı gönderilir.
3. **Çözüm Önerisi:** Yapay zeka sorunun nedenini açıklar ve çözümü için gereken eylemi (örneğin eksik paketi) JSON biçiminde bildirir.
4. **Onay ve Düzeltme:** Sistem size interaktif bir onay sunar:
   ```text
   ==================================================
          🤖 YAPAY ZEKA (OLLAMA) HATA TEŞHİSİ        
   ==================================================
   Teşhis: Program, çalışması için fontconfig kütüphanesini bulamadı. Bu kütüphane eksik olduğunda bu hata oluşur.

   ? Yapay zeka, izole ortam içine 'fontconfig' paketini kurarak çözmeyi öneriyor.
   Kurulsun mu? (y/N) › y
   ```
5. **Yeniden Başlatma:** Siz onay verdiğinizde paket otomatik kurulur ve `izole` uygulamayı otomatik olarak yeniden çalıştırır.

---

## 📄 Lisans
Bu proje MIT lisansı altında lisanslanmıştır. Detaylar için `LICENSE` dosyasına bakabilirsiniz.
