# CI, release ve kod imzalama

Bu rehber, [GitHub Actions workflow'unun](.github/workflows/ci.yml) nasıl çalıştığını ve
macOS / Windows imzalama kurulumunu hatırlamak içindir. Gerçek sertifika, özel anahtar,
parola veya token değerlerini bu dosyaya yazma.

## CI ne zaman çalışır?

- `main` branch'ine push: testler ve Windows, Linux, macOS build'leri çalışır.
- Pull request: aynı kontroller çalışır; fork PR'larına repository secret'ları verilmez.
- `v*` etiketi push: bütün işler başarılı olursa GitHub Release oluşturulur ve dosyalar yüklenir.
- macOS Developer ID imzalama ve notarization yalnızca `v*` tag'lerinde yapılır. Release'te
  beş Apple secret'ı da zorunludur; eksikse imzasız release yayımlamak yerine CI durur.
  Normal `main` / PR build'leri ad-hoc imzalıdır; Apple'a başvuru gönderilmez.
- Normal branch build'lerinin dosyaları Actions çalışmasının **Artifacts** bölümündedir.
- Secret eklemek tek başına build başlatmaz. Sonraki push'u bekle veya uygun bir önceki
  çalışmayı yeniden çalıştır. Ana CI'da `workflow_dispatch` yok; yalnızca notarization
  kurtarma akışı manuel başlatılabilir (aşağıya bak).

Release dosyaları:

| Dosya | Kullanım |
| --- | --- |
| `dost.dmg` | macOS kurulumu; Apple Silicon + Intel universal uygulama |
| `dost_Darwin_arm64.tar.gz` | macOS Apple Silicon uygulaması ve güncelleyicisi |
| `dost_Darwin_x86_64.tar.gz` | macOS Intel uygulaması ve güncelleyicisi |
| `dost_Windows_x86_64.zip` | Windows Intel / AMD indirme ve güncelleme |
| `dost_Windows_arm64.zip` | Windows ARM64 indirme ve güncelleme |
| `dost_Linux_x86_64.tar.gz` | Linux x86_64 indirme ve güncelleme |
| `dost_Linux_arm64.tar.gz` | Linux ARM64 indirme ve güncelleme |
| `dost-<version>-checksums.txt` | Yukarıdaki yedi dosyanın SHA-256 listesi |

Windows ve Linux x86_64 / ARM64 için ayrı, native GitHub-hosted runner'larda derlenir.
Windows ARM64 runner etiketi `windows-11-arm`, Linux ARM64 etiketi `ubuntu-24.04-arm`.
Bu runner'ların repo / hesap planındaki kullanılabilirliğini kontrol et. macOS'ta DMG için
iki mimari `lipo` ile tek universal uygulamada birleştirilir; DMG adı daima `dost.dmg` kalır.
Tar.gz arşivlerinde ise yalnızca ilgili mimariyi içeren `Dost.app` vardır; böylece indirme
ve otomatik güncellemeler daha küçük olur. Güncelleyici çalışan binary'nin mimarisini
seçer; Rosetta altında çalışan Intel sürümü Intel arşivini indirir.
Windows ve Linux arşivlerinin kökünde `dost.exe` / `dost`, `README.md` ve `LICENSE` bulunur.

Her mimarinin macOS uygulaması ayrı oluşturulur ve imzalanır; yalnızca universal uygulamanın
onayının diğer çıktılara da uyduğunu varsaymak yerine hepsi aynı başvuruya eklenir.
Apple `tar.gz` kabul etmediği için notarization'a geçici bir ZIP gönderilir.
Onaydan sonra uygulamaya ticket staple edilir,
son arşiv macOS `tar` ile oluşturulur. CI arşivi yeniden açıp mimari, imza ve (notarize
edilmişse) ticket / Gatekeeper doğrulaması yapar. Güncelleyici de yeni uygulamanın imzasını
kontrol etmeden mevcut uygulamayı değiştirmez. Mevcut Apple secret'ları yeterlidir.

### Tek başvuru ve kaldığı yerden devam

Release'te önce son imzalı universal DMG ve iki mimarinin uygulaması hazırlanır.
`scripts/notarize_macos.py prepare` hepsini tek geçici ZIP içine koyar:

```text
submission.zip
└── payload/
    ├── dost.dmg
    ├── arm64/Dost.app
    └── x86_64/Dost.app
```

Apple iç içe DMG'yi ve içindeki universal uygulamayı da tarar; **üç çıktı için tek başvuru**
yapılır. DMG sonradan yeniden oluşturulmaz, uygulamalar yeniden imzalanmaz.
İmzalı girdiler önce `macos-notary-inputs-<attempt>` artifact'ına yüklenir. Ardından
`submit --no-wait` ile başvuru gönderilir; dönen UUID hemen
`macos-notary-submission-<attempt>` artifact'ına kaydedilir. İki artifact da 30 gün saklanır.
Sertifika, özel anahtar ve parolalar bu dosyalarda bulunmaz.

Asıl bekleme ve yayın [Complete notarized release](.github/workflows/notarization.yml)
akışında gerçekleşir. Bu akış tekrar derleme, imzalama veya başvuru yapmaz:

1. Kaynak CI'ın bu repodaki bir `v*` tag push'u olduğunu ve tag'in hâlâ aynı commit'e
   işaret ettiğini kontrol eder; altı build/test işinin başarılı olmasını şart koşar.
2. İmzalı ZIP'in SHA-256'sını, tag / commit / run ID / attempt bilgisini ve UUID kaydını eşleştirir.
3. Aynı UUID'nin durumunu her dakika sorgular; `Accepted` dışında bir son durumda durur.
4. Apple analiz günlüğünde DMG'nin ve dört uygulama mimarisinin beklenen cdhash / yol /
   mimari kayıtlarını kontrol eder. Birinin kapsamı eksikse yayın yapılmaz.
5. DMG'ye ve iki uygulamaya ticket iliştirir, doğrular; son tarball'ları oluşturup tekrar açar
   ve imza / mimari / ticket / Gatekeeper kontrolü yapar.
6. Kaynak run'ın Windows/Linux arşivlerini birleştirip checksum'ları en son hesaplar ve
   yalnızca orijinal tag için release yayımlar.

Sonuçlar `notarization-results-<run>-<attempt>` artifact'ında saklanır. Ek süre sınırı
tanımlanmaz; GitHub-hosted job'ların 6 saatlik platform sınırı yine geçerlidir. Runner'ın
süresi dolsa bile Apple başvurusu devam eder.

**Devam ettirmek için:** GitHub → Actions → **Complete notarized release** → **Run workflow**
(güncel `main` branch'ini seç):

- `source_run_id`: İlk **tag CI çalışmasının** URL'sindeki sayı.
- `source_attempt`: O kaynak çalışmanın attempt numarası; ilk denemeyse `1`.

Bu değerler kurtarma çalışmasının ID'si değil, imzalı girdilerin ve UUID'nin saklandığı
orijinal CI'a aittir. Artifact'lar mevcut olduğu sürece aynı UUID ile tekrar devam edilir;
6 saat daha beklemek için yeniden dosya yüklenmez. Aynı kaynağın eşzamanlı kurtarmaları
sıraya alınır, birbirini iptal etmez. **Ana macOS build job'unu yeniden çalıştırmak ise
yeniden derleyip yeni başvuru yapar; kurtarma için onu kullanma.**

Bu özellik eski sıralı `v0.2.3` gibi kurtarma artifact'ı üretmeyen çalışmalara geriye dönük
uygulanamaz. UUID artifact'ı yüklenmeden hata olursa `notarytool history` ile mevcut
başvuruyu araştır; körlemesine yeniden gönderme. İmzalı girdiler ve UUID ikisi de gerekli.

Kaynaklar: [Apple'ın çoklu öğe / iç içe kapsayıcı rehberi](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow),
[imzayı koruyabilen mimari çıkarma işlemleri](https://developer.apple.com/library/archive/technotes/tn2206/_index.html#//apple_ref/doc/uid/DTS40007919-CH1-TNTAG402).
Tek başvuru Apple'ın kendi kuyruk süresinin kısa olacağını garanti etmez.

**Eski sürümden geçiş:** `dost-windows.exe`, `Dost-macos.zip`, `dost-linux-x86_64` ve
ayrı `.sha256` dosyalarını arayan eski kurulumların bir kez elle güncellenmesi gerekir.
Eski isimler temiz release listesi için yayımlanmaz. Yeni güncelleyici doğru mimarinin
arşivini seçer ve ortak checksum dosyasında tam dosya adına göre doğrular.

Release işi `scripts/release.py finalize` ile dosya listesinin eksiksizliğini ve etiketin
`Cargo.toml` sürümüyle eşleşmesini kontrol eder. Ardından checksum dosyası ve indirme
tablosu oluşturulur; GitHub'ın otomatik changelog'u release notlarına eklenir.

GoReleaser'ın Rust builder'ı da kullanılabilir; ancak şu an native Windows MSVC build'leri
ve macOS uygulama / DMG imzalama akışını zaten Actions yönetiyor. Yalnızca arşiv, checksum
ve release notları için ek bir araç eklenmedi. Homebrew / Scoop gibi dağıtım kanalları
genişlerse GoReleaser yeniden değerlendirilebilir.

Normal macOS branch build'leri ad-hoc imzalıdır; release'lerde Apple secret'ları zorunludur.
Windows secret'ları yoksa Windows imzasız derlenir. Linux için
bu workflow'da kod imzalama secret'ı yoktur. Release yayımlamak için ayrıca PAT veya
`GITHUB_TOKEN` secret'ı eklemek gerekmez; Actions token'ı otomatik sağlar.

## GitHub secret'ları nereye eklenir?

Repo → **Settings → Secrets and variables → Actions → New repository secret**.

Aşağıdaki isimleri birebir kullan. Mevcut workflow bunları `secrets.*` üzerinden okuduğu
için **Variables** bölümüne ekleme. Environment secret kullanmak istersen workflow'da
ilgili environment'ın da tanımlanması gerekir; mevcut kurulum repository secret kullanır.

## macOS: Developer ID Application ve notarization

### 1. Apple hesabı ve Mac

- Ücretli **Apple Developer Program** üyeliği aktif olmalı; ücretsiz developer hesabı yetmez.
- Mac'in iCloud hesabı farklı olabilir. iCloud'dan çıkış yapmak veya Mac'in hesabını
  değiştirmek gerekmez. Developer sitesine üyeliği olan hesapla giriş yapmak yeterlidir.
- CSR oluşturma, sertifikayı içe aktarma ve `.p12` dışa aktarma işlemlerini aynı Mac'te,
  aynı macOS kullanıcı oturumunda yap. Özel anahtar bu kullanıcıya ait anahtar zincirinde oluşur.

### 2. CSR oluştur

1. **Keychain Access / Anahtar Zinciri Erişimi** uygulamasını aç; Passwords / Parolalar değil.
2. Menü: **Keychain Access → Certificate Assistant → Request a Certificate From a
   Certificate Authority…**
3. **User Email Address:** Developer hesabının e-postası.
4. **Common Name:** ad soyad. **CA Email Address:** boş.
5. **Saved to disk** seç ve `.certSigningRequest` dosyasını kaydet.

Bu işlem özel anahtarı Mac'te oluşturur. CSR dosyası özel anahtarın kendisi değildir.

### 3. Apple'da sertifikayı oluştur

1. [Certificates](https://developer.apple.com/account/resources/certificates/list)
   sayfasına Developer üyeliği olan hesapla giriş yap.
2. **+ → Developer ID Application** seç. Apple Development, Apple Distribution veya
   Developer ID Installer seçme.
3. Ara sertifika seçimi sunulursa **G2 Sub-CA** kullanabilirsin; güncel macOS CI ortamı uygundur.
4. CSR dosyasını yükle, **Generate → Download** ile `.cer` dosyasını indir.

Seçenek kullanılamıyorsa üyeliğin aktif olduğunu ve hesabın sertifika oluşturma yetkisini
kontrol et. Organizasyon hesaplarında Account Holder'ın işlemi yapması gerekebilir.

### 4. Sertifika ile özel anahtarı eşleştir

1. Keychain Access'te sol taraftan **login / oturum açma** anahtar zincirini seç.
2. **File → Import Items… / Dosya → Öğeleri İçe Aktar…** ile `.cer` dosyasını buraya aktar.
   Çift tıklamak sertifikayı Sistem'e ekliyorsa bu açık içe aktarma yolunu kullan.
3. **My Certificates / Sertifikalarım** bölümünde
   `Developer ID Application: Ad Soyad (TEAMID)` kaydını bul.
4. Yanındaki oku açınca **private key / özel anahtar** görünmeli.

Ok veya özel anahtar yoksa:

- Doğru anahtar zincirine ve **Sertifikalarım** bölümüne baktığını kontrol et.
- **login → Keys / Anahtarlar** bölümünde CSR oluşturulurken üretilen anahtarı ara.
- CSR başka Mac'te ya da kullanıcı oturumunda oluşturulduysa özel anahtar oradadır.
  Sertifikayı o ortamda anahtarla birleştirip `.p12` olarak dışa aktar.
- Özel anahtar kayıpsa `.cer` dosyasından geri üretilemez; yeni CSR ve sertifika gerekir.
- **Always Trust / Her Zaman Güven** seçmek eksik anahtarı düzeltmez.
- Sistem'deki kopyayı sırf bu işlem için silmek gerekmez.

### 5. `.p12` dışa aktar

**Sertifikalarım** içindeki, özel anahtarı eşleşmiş Developer ID Application kaydına
sağ tık → **Export → Personal Information Exchange (.p12)**.

Örneğin `DeveloperID.p12` adıyla kaydet ve güçlü bir dışa aktarma parolası belirle.
Anahtar zinciri erişim izni için sorulan yerel Mac kullanıcı parolası ile bu yeni `.p12`
parolası farklı şeylerdir. `.p12`, özel anahtarı içerdiğinden güvenli bir yerde saklanmalıdır.

Mac Terminal'de dosya yolunu uyarlayarak Base64 çıktısını panoya kopyala:

```sh
base64 -i ~/Downloads/DeveloperID.p12 | pbcopy
```

Base64 şifreleme değildir; bu çıktıyı da özel anahtar gibi koru.

### 6. Apple'ın diğer bilgilerini hazırla

- **Team ID:** [Developer Account](https://developer.apple.com/account/) → Membership
  details bölümündeki 10 karakterlik kimlik.
- **Uygulamaya özel parola:** [Apple Account](https://account.apple.com/) →
  **Sign-In and Security → App-Specific Passwords**. Örneğin `Dost GitHub CI` adıyla oluştur.
  Bu özellik için iki faktörlü doğrulama gerekir. Normal Apple giriş parolasını kullanma.

### 7. macOS secret listesi

| Secret | Değer |
| --- | --- |
| `MACOS_CERT_P12` | Özel anahtarı içeren `.p12` dosyasının Base64 çıktısı |
| `MACOS_CERT_PASSWORD` | `.p12` dışa aktarma parolası |
| `APPLE_ID` | Developer hesabının e-posta adresi |
| `APPLE_TEAM_ID` | 10 karakterlik Team ID |
| `APPLE_APP_PASSWORD` | Apple'ın uygulamaya özel parolası |

G2 için ayrı bir secret gerekmez. `SIGN_ID` de secret olarak eklenmez; CI içe aktardığı
sertifikadan otomatik bulur.

### 8. macOS doğrulama

Release tag'i için Actions → **ci → macos** içinde:

- **Import certificate** başarılı olmalı.
- İmzalı girdi artifact'ı ve UUID artifact'ı kaydedilmeli.
- **release / macos** altında aynı UUID `Accepted` olmalı; log kapsam kontrolü,
  `stapler` ve `spctl` kontrolleri başarılı olmalı.
- Branch / PR çalışmasında Apple adımlarının atlanması normaldir.

Sadece yeşil branch build'i görmek release onayı anlamına gelmez. Release'te eksik
secret'lar artık hataya yol açar. Sertifikanın son kullanma tarihini takip et;
yenilediğinde `.p12` secret'larını da güncelle. Apple uygulamaya özel parolası iptal
olursa `APPLE_APP_PASSWORD` değerini yenile.

## Windows: önce imzalama yöntemini seç

**Dost için şu an tercih: imzasız dağıtım.** Windows secret'larını boş bırak; build ve
release çalışır. Aşağıdaki sağlayıcı seçenekleri ileride ihtiyaç olursa başvurmak içindir.

Apple Developer üyeliği Windows imzası sağlamaz. GitHub'dan indirilen bağımsız `.exe`
dosyası için gereken imza **Authenticode code signing** imzasıdır. Microsoft Store
hesabı açmak tek başına bu `.exe` dosyasını imzalamaz.

**Önemli:** Mevcut CI yalnızca özel anahtarı içeren, kullanılabilir bir `.pfx` dosyasıyla
`signtool /f` imzalamayı destekler. Güncel, herkese güvenilen code signing sertifikalarında
özel anahtar genellikle USB token, HSM veya bulut hizmetinde tutulur ve dışa aktarılamaz.
Bu yüzden sertifika satın almadan önce GitHub-hosted Actions desteğini doğrula.

### Seçenek A: SignPath Foundation — açık kaynak için ilk değerlendirilecek yol

Dost MIT lisanslıdır. Proje programın şartlarını karşılar ve başvuru kabul edilirse
[SignPath Foundation](https://signpath.org/) açık kaynak projelere ücretsiz imzalama sunar.
Kabul otomatik veya garanti değildir.

1. [Başvuru sayfasından](https://signpath.org/apply) proje için başvur.
2. Repo adresini, lisansı ve build / release sürecini hazırla.
3. Kabulden sonra SignPath'te proje, imzalanacak artifact ve signing policy ayarlarını yap.
4. GitHub Actions entegrasyonunu kur. Build çıktısı SignPath'e gönderilip imzalı çıktı
   geri alınmalı; SHA-256 ve release yükleme işlemleri **bu imzalı dosya üzerinden** yapılmalı.
5. Entegrasyonun gerektirdiği API token'ını GitHub Actions secret olarak sakla;
   organization / project / policy kimliklerini entegrasyona göre tanımla.

Bu yolda `.pfx` verilmez; `WINDOWS_CERT_PFX` ve `WINDOWS_CERT_PASSWORD` kullanılmaz.
**Mevcut workflow'a SignPath entegrasyonu henüz eklenmiş değildir.** Sadece hesap açmak
ve token eklemek imzalamayı etkinleştirmez.

### Seçenek B: Azure Artifact Signing — ücretli bulut imzalama

Microsoft'un hizmeti eski dokümanlarda **Trusted Signing**, güncel dokümanlarda
**Artifact Signing** olarak geçebilir.

1. Önce [güncel ön koşullardan](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart#prerequisites)
   ülke ve bireysel / kurumsal kimlik doğrulama uygunluğunu kontrol et. Azure bölgesini
   seçebilmek, yaşadığın ülkenin kimlik doğrulama için desteklendiği anlamına gelmez.
2. Ücretli Azure aboneliği ve Artifact Signing hesabı oluştur; maliyetleri kontrol et.
3. Kimlik doğrulamasını tamamla ve **Public Trust** certificate profile oluştur.
4. GitHub Actions kimliği için mümkünse **OIDC / federated credentials** yapılandır;
   yalnızca gerekli profile imzalama yetkisi ver.
5. [Resmî GitHub Action entegrasyonunu](https://github.com/azure/artifact-signing-action)
   workflow'a ekle. OIDC için `id-token: write`, Azure giriş adımı ve hesap / profil /
   endpoint ayarları gerekir. Güveni belirli repo ve release ref / environment'ı ile sınırla.
6. İmzalı çıktıyı doğrula; SHA-256'yı imzalamadan sonra oluştur.

OIDC kurulumunda uzun ömürlü client secret gerekmez. Client ID, tenant ID ve subscription
ID gibi tanımlayıcılar ile signing endpoint / account / profile bilgileri gerekir;
bunların isimleri seçilen workflow'a göre belirlenir ve çoğu Actions variable olabilir.

Bu yolda da `.pfx` kullanılmaz. **Mevcut workflow'a Azure entegrasyonu henüz eklenmemiştir.**

### Seçenek C: bir sertifika otoritesinden code signing hizmeti

DigiCert, Sectigo ve benzeri sağlayıcılarda şu soruları satın almadan önce sor:

- Bireysel geliştirici ve bulunduğum ülke destekleniyor mu?
- GitHub-hosted Windows runner'dan etkileşimsiz imzalama yapılabiliyor mu?
- USB token / yerel makine zorunlu mu, yoksa bulut HSM / API var mı?
- CI entegrasyonu ve toplam maliyet nedir?

USB token gerektiren sertifika için genellikle token'ın bağlı olduğu self-hosted runner
veya farklı bir uzaktan imzalama kurulumu gerekir. Normal GitHub-hosted runner'a USB
token takılamaz. Bulut hizmetinde sağlayıcının entegrasyonu workflow'a eklenmelidir.

### Elinde gerçekten uygun bir `.pfx` varsa: mevcut CI secret'ları

Bu bölüm yalnızca `.pfx` içinde **özel anahtar da mevcutsa**, kullanım koşulları bu
kuruluma izin veriyorsa ve sertifika hedef Windows sistemlerinde güvenilir ise geçerlidir.
Yalnızca public sertifikayı içeren `.pfx` yeterli değildir. Self-signed sertifika genel
kullanıcılardaki güven / SmartScreen sorununu çözmez.

| Secret | Değer |
| --- | --- |
| `WINDOWS_CERT_PFX` | Özel anahtarlı `.pfx` dosyasının Base64 çıktısı |
| `WINDOWS_CERT_PASSWORD` | `.pfx` parolası |

Mac'te:

```sh
base64 -i ~/Downloads/cert.pfx | pbcopy
```

Windows PowerShell'de:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("$HOME\Downloads\cert.pfx")) | Set-Clipboard
```

### Windows imzasını doğrulama

İndirilen release dosyasında PowerShell ile:

```powershell
Get-AuthenticodeSignature .\dost.exe | Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
```

`Status: Valid` beklenir; yayıncı ve zaman damgası bilgilerini de kontrol et.
Explorer → dosyaya sağ tık → **Properties → Digital Signatures** bölümünden de bakılabilir.

**Geçerli imza, SmartScreen uyarısının anında kalkacağını garanti etmez.** SmartScreen
dosya / yayıncı itibarı gibi ek sinyalleri değerlendirir. İmza ile itibar aynı şey değildir.

## Release kontrol listesi

1. Gerekli secret'ları ayarla; gerçek değerleri commit etme.
2. Sürümü `Cargo.toml` içinde güncelle; gerekiyorsa `Cargo.lock` dosyasını da güncelle.
3. Değişiklikleri commit edip `main` branch'ine gönder.
4. CI'da bütün platformların başarılı olduğunu ve beklenen imzalama adımlarının
   atlanmadığını kontrol et.
5. Sürümle eşleşen, daha önce kullanılmamış bir `v…` etiketi gönder. Örnek olarak
   sürüm gerçekten `0.2.0` ise:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

6. Etiketin başlattığı CI ve release işini kontrol et. Release dosyalarını indirip
   macOS notarization / Windows imzasını ayrıca doğrula.

Checksum kontrolü için istediğin arşivi ve `dost-<version>-checksums.txt` dosyasını aynı
klasöre indir. Linux'ta örneğin:

```sh
sha256sum --check --ignore-missing dost-0.2.3-checksums.txt
```

Mac'te `shasum -a 256 dost.dmg`, Windows'ta
`Get-FileHash .\dost_Windows_x86_64.zip -Algorithm SHA256` çıktısını listedeki doğru
dosyanın satırıyla karşılaştır. Örnek sürüm numarasını gerçek sürümle değiştir.

Şu an Windows sağlayıcısı seçilmediyse ilgili secret'ları boş bırakabilirsin; Windows
build ve release yine üretilir, ancak `.exe` imzasız olur.

## Kaynaklar

- [Apple: Developer ID sertifikaları](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/)
- [Apple: uygulamaya özel parolalar](https://support.apple.com/102654)
- [SignPath Foundation](https://signpath.org/)
- [Azure Artifact Signing genel bakış](https://learn.microsoft.com/en-us/azure/artifact-signing/overview)
- [Azure Artifact Signing entegrasyonları](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations)
- [Azure Artifact Signing SSS ve SmartScreen](https://learn.microsoft.com/en-us/azure/artifact-signing/faq)

Windows hizmet bilgileri son kontrol: 23 Ekim 2026. Başvuru, ülke desteği ve fiyatlar
değişebileceğinden hizmeti seçerken resmî kaynakları tekrar kontrol et.
