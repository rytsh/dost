//! The app's own text in English and Turkish. Text is written in English in the code and
//! looked up here: `t("Size")` is "Boyut" while the language is Turkish. Missing entries stay
//! English. Placeholders like `{n}` are filled in by the caller.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::Lang;

/// The language in use (0 English, 1 Turkish).
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Sets the language for [`t`]; `Auto` follows the system.
pub fn set(lang: Lang) {
    let tr = match lang {
        Lang::Tr => true,
        Lang::En => false,
        Lang::Auto => system_is_turkish(),
    };
    CURRENT.store(tr as u8, Ordering::Relaxed);
}

/// Whether the text is Turkish now.
pub fn is_turkish() -> bool {
    CURRENT.load(Ordering::Relaxed) == 1
}

/// `text` in the current language.
pub fn t(text: &str) -> &str {
    if is_turkish() { turkish(text) } else { text }
}

/// `text` in Turkish, or itself when there is no translation.
pub fn turkish(text: &str) -> &str {
    static MAP: std::sync::OnceLock<std::collections::HashMap<&str, &str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| TR.iter().copied().collect())
        .get(text)
        .copied()
        .unwrap_or(text)
}

/// Whether the system's language is Turkish (checked once).
pub fn system_is_turkish() -> bool {
    static SYSTEM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SYSTEM.get_or_init(|| system_lang().is_some_and(|l| l.to_lowercase().starts_with("tr")))
}

#[cfg(not(windows))]
fn env_lang() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
}

#[cfg(target_os = "macos")]
fn system_lang() -> Option<String> {
    // Apps started from Finder have no LANG; the user's preferred languages are in defaults.
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split(|c: char| c == '"' || c == '(' || c == ',' || c.is_whitespace())
        .find(|s| s.len() >= 2 && s.chars().next().is_some_and(char::is_alphabetic))
        .map(String::from)
        .or_else(env_lang)
}

#[cfg(windows)]
fn system_lang() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    // The primary language id is the low 10 bits; Turkish is 0x1f.
    let id = unsafe { GetUserDefaultUILanguage() };
    Some(if id & 0x3ff == 0x1f { "tr" } else { "en" }.into())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_lang() -> Option<String> {
    env_lang()
}

/// English -> Turkish.
static TR: &[(&str, &str)] = &[
    ("+{n} more", "+{n} daha"),
    ("1 question waiting", "1 soru bekliyor"),
    (
        "A bot token from @BotFather.",
        "@BotFather'dan alınan bot jetonu.",
    ),
    ("About", "Hakkında"),
    ("Add", "Ekle"),
    ("Agents", "Ajanlar"),
    (
        "Agents can add items and reminders over MCP.",
        "Ajanlar MCP ile öğe ve hatırlatıcı ekleyebilir.",
    ),
    ("All settings…", "Tüm ayarlar…"),
    ("Allow", "İzin"),
    ("Always", "Her zaman"),
    ("Answer", "Cevap"),
    ("Any", "Herhangi"),
    ("App", "Uygulama"),
    ("Appearance", "Görünüm"),
    ("Asleep", "Uykuda"),
    ("Away", "Uzaktayken"),
    ("Awake", "Uyanık"),
    ("Bark on notifications", "Bildirimlerde havla"),
    ("Bark sounds", "Havlama sesleri"),
    ("Bark when petted", "Sevilince havla"),
    ("Basics", "Temel"),
    ("Beanie", "Bere"),
    ("Birthday", "Doğum günü"),
    ("Bone", "Kemik"),
    ("Browsers", "Tarayıcılar"),
    ("Bubble", "Balon"),
    ("Bubble keys", "Balon tuşları"),
    ("Bubble stays", "Balon süresi"),
    ("Cancel", "Vazgeç"),
    ("Check for updates", "Güncellemeleri denetle"),
    ("Classic", "Klasik"),
    ("Clear all", "Hepsini kapat"),
    ("Clear done", "Bitenleri sil"),
    (
        "Click a colour to change it.",
        "Değiştirmek için renge tıkla.",
    ),
    ("Click to open ↗", "Açmak için tıkla ↗"),
    ("Collar", "Tasma"),
    ("Commemoration", "Anma"),
    ("Connect", "Bağlantı"),
    ("Copy", "Kopyala"),
    (
        "Could not change autostart",
        "Otomatik başlatma değiştirilemedi",
    ),
    ("Could not check for updates", "Güncellemeler denetlenemedi"),
    ("Dark", "Koyu"),
    ("Deny", "Reddet"),
    ("Dog", "Köpek"),
    ("Done", "Bitti"),
    (
        "Dost can reach your phone.",
        "Dost telefonuna ulaşabiliyor.",
    ),
    ("Dost is up to date", "Dost güncel"),
    (
        "Drag it out, or click to throw.",
        "Dışarı sürükle ya da fırlatmak için tıkla.",
    ),
    (
        "Every day at the same time no quiet hours.",
        "Aynı saat: sessiz saat yok.",
    ),
    ("Failed", "Başarısız"),
    ("Fast", "Hızlı"),
    ("Fetch", "Getir"),
    ("From", "Başlangıç"),
    ("Fur", "Tüy"),
    ("General", "Genel"),
    ("Hidden", "Gizli"),
    ("Hidden · asleep", "Gizli · uykuda"),
    ("Hide", "Gizle"),
    ("History", "Geçmiş"),
    ("Holiday outfits", "Bayram kıyafetleri"),
    (
        "Holiday outfits · Christmas",
        "Bayram kıyafetleri · Yılbaşı",
    ),
    (
        "Holiday outfits · Halloween",
        "Bayram kıyafetleri · Cadılar Bayramı",
    ),
    (
        "Holiday outfits · birthday today!",
        "Bayram kıyafetleri · bugün doğum günü!",
    ),
    (
        "Install updates automatically",
        "Güncellemeleri kendiliğinden kur",
    ),
    ("Language", "Dil"),
    ("Later · 10 min", "Sonra · 10 dk"),
    ("Local", "Yerel"),
    ("Look", "Görünüm"),
    ("MCP address", "MCP adresi"),
    ("MCP server", "MCP sunucusu"),
    ("MCP server could not start", "MCP sunucusu başlatılamadı"),
    ("Main", "Ana ekran"),
    ("Mint", "Nane"),
    ("Name", "Ad"),
    ("Never", "Asla"),
    ("New list", "Yeni liste"),
    (
        "No agents yet. They show up here while they work.",
        "Henüz ajan yok. Çalışırken burada görünürler.",
    ),
    (
        "No lists yet. Make one below, or ask an agent:",
        "Henüz liste yok. Aşağıdan oluştur ya da bir ajana sor:",
    ),
    ("None", "Yok"),
    ("Normal", "Normal"),
    (
        "Nothing yet. Notifications show up here.",
        "Henüz bir şey yok. Bildirimler burada görünür.",
    ),
    ("Offset", "Kaydırma"),
    ("Open at login", "Oturum açınca başlat"),
    ("Start with Windows", "Windows ile başlat"),
    ("Start at login", "Oturum açınca başlat"),
    ("Notifications", "Bildirimler"),
    ("Work", "Çalışma"),
    (
        "Happy, sad and asking voices",
        "Mutlu, üzgün ve soran sesler",
    ),
    (
        "16-bit WAV files; one is picked at random.",
        "16 bit WAV dosyaları; rastgele biri seçilir.",
    ),
    (
        "“make a todo list for the release”.",
        "“sürüm için bir yapılacaklar listesi yap”.",
    ),
    ("Address", "Adres"),
    ("Custom", "Özel"),
    (
        "Offset raises the dog (−20 to 2000).",
        "Kaydırma köpeği yükseltir (−20 ile 2000).",
    ),
    ("Open settings file", "Ayar dosyasını aç"),
    ("Out to play with", "Oyuncak dışarıda"),
    (
        "Pages on localhost can always connect.",
        "localhost sayfaları her zaman bağlanabilir.",
    ),
    ("Phone", "Telefon"),
    ("Play", "Oyun"),
    ("Private network access", "Özel ağ erişimi"),
    ("Pumpkin", "Balkabağı"),
    (
        "Put away. Drag it out to play.",
        "Kaldırıldı. Oynamak için dışarı sürükle.",
    ),
    ("Quiet", "Sessiz"),
    ("Quiet in full screen", "Tam ekranda sessiz"),
    ("Quiet now", "Şimdi sessiz"),
    ("Quit", "Çık"),
    ("Reply", "Yanıtla"),
    ("Reply…", "Yanıtla…"),
    (
        "Right-click it to put it away.",
        "Kaldırmak için sağ tıkla.",
    ),
    (
        "Same From and To: no quiet hours.",
        "Başlangıç ve bitiş aynı: sessiz saat yok.",
    ),
    ("Screen", "Ekran"),
    ("Screen edge", "Ekran kenarı"),
    ("Send", "Gönder"),
    ("Send a test", "Deneme gönder"),
    ("Server", "Sunucu"),
    ("Settings", "Ayarlar"),
    ("Settings file", "Ayar dosyası"),
    ("Show", "Göster"),
    ("Size", "Boyut"),
    ("Sky", "Gök"),
    ("Sleep", "Uyu"),
    ("Sleeping", "Uyuyor"),
    ("Slow", "Yavaş"),
    ("Sound", "Ses"),
    ("Source colours", "Kaynak renkleri"),
    ("Special days", "Özel günler"),
    ("Speed", "Hız"),
    ("Stand on", "Durduğu yer"),
    ("System", "Sistem"),
    ("Taskbar", "Görev çubuğu"),
    ("Test", "Dene"),
    ("Text size", "Yazı boyutu"),
    ("To", "Bitiş"),
    ("Todo", "Yapılacaklar"),
    ("Token", "Jeton"),
    ("Topic", "Konu"),
    ("Toys", "Oyuncaklar"),
    ("Until clicked", "Tıklanana kadar"),
    ("Volume", "Ses düzeyi"),
    ("Wake", "Uyandır"),
    ("Walk around", "Dolaş"),
    ("Walking", "Yürüyüş"),
    ("Wears", "Giydiği"),
    ("When", "Ne zaman"),
    ("While you were away", "Sen yokken"),
    ("Woof!", "Hav!"),
    ("a new item", "yeni bir öğe"),
    ("a source, e.g. CI", "bir kaynak, ör. CI"),
    ("done", "bitti"),
    ("e.g. Release 0.3", "ör. Sürüm 0.3"),
    ("failed", "başarısız"),
    ("idle", "boşta"),
    ("new here", "yeni geldi"),
    ("path to a .wav file", ".wav dosyasının yolu"),
    ("waiting for you", "seni bekliyor"),
    ("working", "çalışıyor"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for (en, _) in TR {
            assert!(seen.insert(en), "{en:?} twice");
        }
    }

    #[test]
    fn lookup() {
        assert_eq!(turkish("Size"), "Boyut");
        assert_eq!(turkish("no such text"), "no such text");
    }
}
