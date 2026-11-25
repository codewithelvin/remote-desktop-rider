(function () {
  "use strict";

  var AZ = {
    "meta.desc": "Windows üçün sürətli və məxfi 1:1 uzaq masaüstü. Zəif bağlantıda belə rahat işləyir. Uçdan-uca şifrələmə, hesab və bulud xidməti tələb olunmur, server sizin özünüzdədir.",
    "skip": "Əsas məzmuna keçin",
    "brand.label": "Remote Desktop Rider, səhifənin əvvəlinə qayıdın",
    "nav.label": "Bölmələr",
    "lang.label": "Dil",
    "gh.label": "Remote Desktop Rider GitHub-da",
    "nav.features": "İmkanlar",
    "nav.how": "Necə işləyir",
    "nav.security": "Təhlükəsizlik",
    "nav.selfhost": "Öz serveriniz",
    "nav.faq": "Suallar",
    "nav.download": "Yükləmə",
    "nav.enterprise": "Korporativ",

    "hero.eyebrow": "Açıq mənbə · Windows, macOS və Linux · Rust dilində yazılıb",
    "hero.tagline": "Windows üçün sürətli və məxfi 1:1 uzaq masaüstü. Zəif bağlantıda belə rahat işləyir.",
    "hero.lede": "Ailə üzvünüzə kömək edin, evdən ofis kompüterinizə qoşulun və ya müştərinizə dəstək göstərin: mobil internet, otel Wi-Fi şəbəkəsi, ciddi məhdudiyyətləri olan korporativ şəbəkə və ya yavaş VPN üzərindən.",
    "cta.download.windows": "Windows üçün yükləyin",
    "cta.download.mac": "macOS üçün yükləyin",
    "cta.download.linux": "Linux üçün yükləyin",
    "cta.download.any": "Yükləyin",
    "cta.other": "Digər sistemlər",
    "cta.github": "GitHub-da baxın",
    "hero.fine": "Pulsuz və açıq mənbəli. Windows, macOS və ya Linux-dan idarə edin; idarə olunan kompüterdə Windows 10/11 olmalıdır. Hesab və bulud xidməti tələb olunmur.",
    "hero.alt": "Remote Desktop Rider-də uzaq seans: uzaq Windows masaüstü baxış pəncərəsini tam tutur, yuxarıda ortada seans alətlər panelinin nişanı görünür.",

    "features.title": "Bağlantı zəif olsa belə, işləyir",
    "f1.h": "Zəif şəbəkədə də axıcı",
    "f1.p": "UDP üzərindən QUIC, aparat səviyyəsində H.264 kodlaşdırması (QuickSync / NVENC / AMF), lokal çəkilən kursor, köhnəlmiş kadrları təkrar göndərmək əvəzinə ötürməmək və real vaxtda adaptiv bitreyt (300&nbsp;kbps–20&nbsp;Mbps).",
    "f2.h": "Keyfiyyəti özünüz seçin",
    "f2.p": "Seans alətlər panelində Auto / 1080p / 720p / 480p. Auto rejimi yavaş bağlantıda ayırdetməni azaldır, bağlantı bərpa olunduqda isə yenidən artırır.",
    "f3.h": "Məxfilik əsas prinsipdir",
    "f3.p": "Cihazlar bir dəfə, kodu müqayisə etməklə cütləşdirilir. Hər seans uçdan-uca şifrələnir və kompüterin PIN kodunu tələb edir. Hesab və bulud xidməti yoxdur.",
    "f4.h": "Məhdudiyyətli şəbəkələrdən keçir",
    "f4.p": "Lokal şəbəkədə birbaşa, internetdə isə NAT keçidi (hole punching) və ya şifrələnmiş relay vasitəsilə. Lazım gəldikdə təsadüfi UDP portuna, sonra isə sistem proksisi üzərindən HTTPS (TCP 443) tunelinə keçir.",
    "f5.h": "Uzaq kompüterdə hər yerdə işləyir",
    "f5.p": "Giriş ekranında, kilid ekranında, UAC sorğularında və hətta yenidən başladıldıqdan sonra.",
    "f6.h": "Gündəlik alətlər",
    "f6.p": "Mübadilə buferindəki mətnin hər iki istiqamətdə sinxronlaşdırılması, faylları sürüşdürüb uzaq kompüterə atmaq, monitorlar arasında keçid, Ctrl+Alt+Del, uzaq kompüteri kilidləmək.",
    "f7.h": "Öz serverinizdə",
    "f7.p": "İstəyə bağlı <code>rider-server</code> istənilən kiçik Linux maşınında işləyir. O, cihazları bir-biri ilə tanış edir və şifrələnmiş trafiki ötürür, lakin seanslara baxa və ya onları ələ keçirə bilməz.",
    "f8.h": "Özünü yeniləyir",
    "f8.p": "Hər saat yeniləmə yoxlanışı, SHA-256 ilə yoxlanılan quraşdırıcılar, yalnız uzaqdan qoşulduğunuz kompüterlər üçün istifadəçi müdaxiləsi olmadan yeniləmə.",
    "f9.h": "Yığcam və yerli",
    "f9.p": "Hər sistem üçün yerli tətbiq; Windows quraşdırıcısının həcmi təxminən 8&nbsp;MB-dır. Tamamilə Rust dilində yazılıb.",
    "f10.h": "İngilis və Azərbaycan dillərində",
    "f10.p": "Tətbiq, seans pəncərəsi və idarəetmə pəncərəsi ingilis və Azərbaycan dillərində mövcuddur. Dili tətbiqin yan panelindəki və ya «Parametrlər» bölməsindəki bayraq düymələri ilə dəyişə bilərsiniz.",

    "how.title": "Bir dəfə cütləşdirin, sonra PIN kodla qoşulun",
    "how.lede": "Remote Desktop Rider-i hər iki kompüterə quraşdırın (bax: <a href=\"#download\">Yükləmə</a>). İdarə olunan kompüterdə Windows olmalıdır; onu Windows, macOS və ya Linux-dan idarə edə bilərsiniz. Cütləşdirmə hər cihaz üçün yalnız bir dəfə aparılır; bundan sonra hər seans kompüterin PIN kodunu tələb edir.",
    "s1.h": "İdarə ediləcək kompüterdə",
    "s1.p": "«Bu kompüter» səhifəsində «Bu kompüteri idarə et» pəncərəsini açın, PIN kod təyin edin, «Uzaqdan giriş» funksiyasını aktivləşdirin və «Cihaz əlavə et» düyməsini klikləyin. Ekranda qısa kod görünəcək.",
    "s1.alt": "«Bu kompüter» ekranı: uzaqdan giriş açarı, PIN kod ayarı və cütləşdirilmiş cihazların siyahısı.",
    "s2.h": "Öz cihazınızda",
    "s2.p": "«Kompüter əlavə et» düyməsini klikləyin, həmin kodu daxil edin və hər iki ekranda eyni təsdiq kodunun göründüyünə əmin olun.",
    "s2.alt": "«Kompüterlər» ekranı: bu cihazın idarə edə bildiyi kompüterlərin siyahısı və onların onlayn olub-olmaması.",
    "s3.h": "Qoşulun",
    "s3.p": "Kompüter onlayn olduqda «Qoşul» düyməsini klikləyin və PIN kodu daxil edin. «PIN kodu bu cihazda yadda saxla» seçimini işarələsəniz, növbəti qoşulmalar dərhal açılacaq, digər proqramlardakı daimi (unattended) giriş kimi. Windows-da PIN kod Windows hesabınız üçün şifrələnir. Kimsə qoşulu olduğu müddətdə uzaq kompüterdə qırmızı banner görünür; ona klikləmək seansı bitirir.",
    "s3.alt": "Kompüterin PIN kodunu soruşan qoşulma pəncərəsi və «PIN kodu bu cihazda yadda saxla» seçimi.",
    "how.note": "Fərqli şəbəkələr arasında cütləşdirmə və qoşulma üçün hər kompüterdə bir dəfə <code>rider-server</code> serverinin ünvanını və barmaq izini daxil edin: Windows-da «Bu kompüter» → «Bu kompüteri idarə et» → «Server», macOS və Linux-da isə «Parametrlər» → «Server» bölməsində (bax: <a href=\"#self-host\">Öz serverinizi qurun</a>). Server olmadan da eyni şəbəkədəki cihazlar ünvan vasitəsilə cütləşə bilər.",

    "q.badge": "Yeni",
    "q.title": "Birdəfəlik kodla sürətli qoşulma",
    "q.lede": "Uzunmüddətli giriş üçün deyil, birdəfəlik yardım üçün. Administrator hüquqları, PIN kod və ya cütləşdirmə tələb olunmur.",
    "q.who.helped": "Kömək istəyən şəxs",
    "q.who.helper": "Kömək edən şəxs",
    "q.f1": "Remote Desktop Rider-i açır və «Bu kompüter» səhifəsində «Birdəfəlik kod al» düyməsini klikləyir.",
    "q.f2": "«Sürətli qoşulma» düyməsini klikləyir və həmin kodu daxil edir.",
    "q.f3": "Kimin qoşulmaq istədiyini və təsdiq kodunu görür, sonra «İcazə ver» (və ya «İmtina et») düyməsini klikləyir.",
    "q.x1": "SF6-CK6 kimi 6 simvollu kodu (hərf və rəqəmlər) server verir. Kod yalnız bir dəfə işləyir və 10 dəqiqə ərzində etibarlıdır.",
    "q.x4": "Sürətli qoşulma üçün server təyin olunmalıdır (Windows-da: «Bu kompüter» → «Bu kompüteri idarə et» → «Server»; macOS və Linux-da: «Parametrlər» → «Server»).",
    "q.x2": "Seansdan sonra heç nə yadda saxlanılmır.",
    "q.x3": "Tərəflərdən biri seansı bağladıqda kömək edən şəxsin görüntüsü kəsilir; uzaq ekrandakı qırmızı banner da seansı bitirir.",
    "q.limit": "<b>Məhdudiyyət:</b> sürətli seans administrator hüquqları olmadan işlədiyi üçün administrator kimi işləyən pəncərələri və UAC sorğularını idarə etmək mümkün deyil. Bunun üçün adi, cütləşdirilmiş girişdən istifadə edin.",

    "c.kicker": "Necə qoşulur",
    "c.title": "İmkan varsa birbaşa, lazım gəldikdə relay ilə, başqa yol qalmadıqda HTTPS ilə",
    "c.svg.title": "Qoşulma yolları",
    "c.svg.desc": "Kompüteriniz mümkün olduqda uzaq kompüterə QUIC/UDP vasitəsilə birbaşa qoşulur. Əks halda trafik rider-server-in UDP relay-i üzərindən, UDP ümumiyyətlə keçmədikdə isə serverə TCP 443 portunda HTTPS tuneli ilə ötürülür.",
    "c.n.you": "Kompüteriniz",
    "c.n.remote": "Uzaq kompüter",
    "c.n.remote.sub": "rider-host xidməti",
    "c.n.server": "Sizin rider-server",
    "c.l1": "birbaşa QUIC/UDP (lokal şəbəkə və ya NAT keçidi)",
    "c.caption": "Üç qoşulma yolunun diaqramı; təsviri aşağıdakı siyahıdadır.",
    "c.p1": "<b>Birbaşa.</b> Eyni şəbəkədə, yaxud server hər iki tərəfə NAT-dan keçməyə kömək etdikdən sonra internet üzərindən. İlk növbədə lokal şəbəkə ünvanları yoxlanılır.",
    "c.p2": "<b>Relay.</b> Heç bir birbaşa yol işləmədikdə seans serverin UDP relay-i üzərindən keçir.",
    "c.p3": "<b>HTTPS tuneli.</b> UDP trafikinin ümumiyyətlə keçmədiyi şəbəkələrdə (bəzi korporativ şəbəkələr) hər iki tərəf trafiki brauzer kimi, sistem proksisi vasitəsilə 443-cü portda HTTPS üzərindən WebSocket daxilində ötürür. Bu halda seanslar relay-dən istifadə edir.",
    "c.note": "İstənilən yolda seans cütləşdirmə zamanı təsdiqlənmiş açarlarla uçdan-uca şifrələnir. Server və relay yalnız şifrələnmiş məlumatı görür. Uzaq kompüterin adi UDP portu 47800 bloklanıbsa, o, əlavə olaraq təsadüfi yüksək nömrəli portdan da istifadə edir. Server əlçatan olmadıqda belə, məlum birbaşa ünvan yenə yoxlanılır. Baxış pəncərəsi hansı yolun seçildiyini göstərir.",

    "sec.title": "Hesab yoxdur, bulud yoxdur, sındırıla biləcək mərkəzi nöqtə yoxdur",
    "sec1.h": "Cütləşdirmə zamanı təsdiqlənən cihaz açarları",
    "sec1.p": "Hər cihazın öz Ed25519 identifikasiya açarı var. Uzaq kompüteri təqlid etmək üçün ələ keçirilə biləcək sertifikat mərkəzi (CA) və ya mərkəzi server yoxdur.",
    "sec2.h": "Cütləşdirmə hər iki tərəfdə təsdiqlənir",
    "sec2.p": "Hər iki ekranda həm açarlardan, <em>həm də</em> aktiv TLS sessiyasından hesablanan 6 rəqəmli təsdiq kodu göstərilir. Araya girən təcavüzkar (man-in-the-middle) fərqli kodlar yaradardı.",
    "sec3.h": "Qarşılıqlı TLS",
    "sec3.p": "Uzaq kompüter cütləşdirilməmiş cihazları hələ TLS əlsıxma mərhələsində, tətbiqin heç bir kodu işə düşməzdən əvvəl rədd edir. Açarı dəyişmiş kompüterə isə müştəri qoşulmur.",
    "sec4.h": "Hər seans üçün PIN kod",
    "sec4.p": "Argon2id ilə heşlənir, hər uğursuz cəhddən sonra gözləmə müddəti eksponensial artır, 10 uğursuz cəhddən sonra isə 15 dəqiqəlik bloklanır. Oğurlanmış noutbuk tək başına kifayət etmir.",
    "sec5.h": "Eyni anda yalnız bir seans",
    "sec5.p": "Kimsə qoşulu olduqda uzaq kompüterdə qırmızı banner görünür. Banner yayımda göstərilmir; ona klikləmək seansı bitirir.",
    "sec6.h": "Server yalnız şifrələnmiş məlumatı görür",
    "sec6.p": "Server uzaq kompüterə baxa, onu ələ keçirə və ya təqlid edə bilməz. Kompüterlə cütləşdirilməmiş hər kəsə eyni cavabı (“oflayndır və ya icazə yoxdur”) verir və sorğuların sayını IP ünvanı üzrə məhdudlaşdırır.",
    "sec7.h": "Qorunan fayllar",
    "sec7.p": "Uzaq kompüterin gizli açarı və etibarlı cihazlar siyahısı <code>%ProgramData%\\RemoteDesktopRider</code> qovluğunda saxlanılır və onları yalnız SYSTEM və Administrators oxuya bilər.",
    "sec8.h": "Yoxlanılan yeniləmələr",
    "sec8.p": "Hər yükləmə buraxılışla birlikdə dərc olunan SHA-256 cəmi ilə yoxlanılır. Avtomatik yeniləmələri söndürmək mümkündür.",
    "ent.title": "Şirkətin bütün kompüterləri, bir administrator, razılıq olmadan heç kimə baxılmır",
    "ent.lede": "Korporativ rejimdə bir İT administratoru şirkətin Windows kompüterlərini tətbiqdən idarə edir: mərtəbə və otaqlar üzrə düzülmüş, axtarıla bilən, iki kliklə qoşulan. Hər şey kimi, bu da sizin öz <code>rider-server</code> serverinizdə işləyir.",
    "ent.alt": "Korporativ ekran: solda mərtəbələr və otaqlar, sağda seçilmiş otağın kompüterləri; hər birində kimin daxil olduğu və onlayn olub-olmadığı göstərilir.",
    "ent1.h": "Group Policy ilə yayılır",
    "ent1.p": "Başlanğıc skripti Remote Desktop Rider-i domendəki hər kompüterə quraşdırır və tokenlə şirkətə qoşur. Yeni kompüterlər <b>Təyin edilməyib</b> bölməsində görünür; onları öz otağına sürüşdürün.",
    "ent2.h": "Qərarı işçi verir",
    "ent2.p": "Hər qoşulmada kompüter arxasındakı şəxsdən <b>Qəbul et</b> və ya <b>Rədd et</b> seçməsi soruşulur. Bir dəqiqə ərzində cavab yoxdursa, ekran kilidlidirsə və ya heç kim daxil olmayıbsa, seans başlamır.",
    "ent3.h": "Nəzarətsiz giriş yalnız hər iki tərəf razı olduqda",
    "ent3.p": "Soruşmadan giriş üçün həm quraşdırıcının <code>/UNATTENDED</code> açarı, <em>həm də</em> administratorun həmin kompüter üçün açarı lazımdır. Təkcə biri kifayət deyil.",
    "ent4.h": "Bir administrator, sabitlənmiş",
    "ent4.p": "Administrator birdəfəlik quraşdırma kodu ilə bir dəfə daxil olur. Şirkət kompüterləri yalnız həmin administratorun cihaz açarına etibar edir; digər cihazlar onları nə görə, nə də onlara qoşula bilər.",
    "ent5.h": "Ofisdə, yoxsa uzaqdan — bir baxışda",
    "ent5.p": "Evdən şirkətin VPN-i (məsələn, GlobalProtect) ilə işləyən noutbuk <b>Uzaqdan</b> kimi işarələnir. Hər kompüter bunu özü müəyyən edir: Windows domeni öz şəbəkəsindədirsə, deməli ofisdədir.",
    "ent.note": "Addım-addım quraşdırma: <a href=\"https://github.com/codewithelvin/remote-desktop-rider/blob/main/docs/ENTERPRISE.md\">İT üçün bələdçi</a>.",

    "sh.kicker": "Öz serverinizi qurun",
    "sh.title": "Öz serveriniz bir əmrlə",
    "sh.lede": "Server olmadan cihazlar bir-birinə yalnız eyni şəbəkədə, VPN üzərindən və ya port yönləndirməsi ilə qoşula bilər. İstənilən yerdən qoşulmaq üçün kiçik <code>rider-server</code> proqramını hər iki tərəfin əlçatan olduğu istənilən Linux maşınında işə salın. Ucuz bir VPS kifayətdir.",
    "sh.cmd.label": "Quraşdırma əmri",
    "sh.cmd2.label": "HTTPS ehtiyat yolu ilə quraşdırma əmri",
    "copy": "Kopyalayın",
    "copy.done": "Kopyalandı",
    "copy.fail": "Ctrl+C basın",
    "copy.status": "Əmr mübadilə buferinə kopyalandı.",
    "sh.c1": "<code>rider-server</code> proqramını (Linux x86_64, systemd) GitHub-dakı son buraxılışdan quraşdırır və ya yeniləyir.",
    "sh.c2": "Onun SHA-256 cəmini yoxlayır.",
    "sh.c3": "ufw-də UDP 47900 və 47901–47999 portlarını açır.",
    "sh.c4": "Yeniləmə zamanı server açarını saxlayır, buna görə barmaq izi heç vaxt dəyişmir.",
    "sh.c5": "Barmaq izini ekrana çıxarır.",
    "sh.ip": "<code>&lt;public-ip&gt;</code> istəyə bağlıdır, yalnız bulud NAT-ı arxasında mütləqdir.",
    "sh.https.h": "İstəyə bağlı: UDP-ni bloklayan şəbəkələr üçün HTTPS ehtiyat yolu",
    "sh.https.p": "nginx və Let's Encrypt sertifikatını da quraşdırmaq üçün əmrə <code>--domain rider.example.com --email you@example.com</code> əlavə edin. Bunun üçün TCP 80 və 443 portları açıq olmalı, DNS adı isə serverə yönəldilməlidir.",
    "sh.then.h": "Sonra hər kompüterdə",
    "sh.then.p": "Serverin ünvanını və barmaq izini daxil edin: Windows-da «Bu kompüter» → «Bu kompüteri idarə et» → «Server», macOS və Linux-da isə «Parametrlər» → «Server» bölməsində «Ünvan» və «Barmaq izi» sahələrini doldurun. Bu ayar hər iki istiqamətdə keçərlidir: həm başqalarının sizə, həm də sizin başqalarına qoşulmanız üçün.",

    "dl.title": "Sisteminiz üçün yükləyin",
    "dl.lede": "Başqa kompüterləri Windows, macOS və ya Linux-dan idarə edin. İdarə olunan kompüter hələlik yalnız Windows ola bilər.",
    "dl.win.req": "Windows 10/11, 64 bit",
    "dl.win.does": "Tam funksional tətbiq: həm başqa kompüterləri idarə edə, həm də bu kompüterə uzaqdan giriş verə bilərsiniz (idarəetmə pəncərəsi, birdəfəlik kodlar, daimi giriş).",
    "dl.win.hint": "Quraşdırıcı hələ rəqəmsal imza ilə imzalanmayıb: SmartScreen xəbərdarlıq etsə, <b>More info → Run anyway</b> seçin.",
    "dl.win.btn": ".exe faylını yükləyin",
    "dl.mac.req": "macOS 11 və daha yeni · Apple Silicon və Intel",
    "dl.mac.does": "Hələlik yalnız başqa kompüterləri idarə edir; bu kompüterə uzaqdan giriş imkanı planlaşdırılır.",
    "dl.mac.hint": ".dmg faylını açın və tətbiqi «Applications» qovluğuna sürüşdürün. Tətbiq hələ Apple Developer ID ilə imzalanmadığı üçün ilk açılış bloklanır: <b>System Settings → Privacy &amp; Security</b> bölməsini açıb <b>Open Anyway</b> düyməsini klikləyin (və ya tətbiqin üzərində sağ klikləyib <b>Open</b> seçin).",
    "dl.mac.btn": ".dmg faylını yükləyin",
    "dl.deb.req": "Debian 12+ / Ubuntu 22.04+ · x86_64",
    "dl.deb.does": "Hələlik yalnız başqa kompüterləri idarə edir.",
    "dl.deb.hint": "Terminalda quraşdırın:",
    "dl.deb.cmd.label": "Debian və Ubuntu üçün quraşdırma əmri",
    "dl.deb.btn": ".deb faylını yükləyin",
    "dl.srv.h": "Server",
    "dl.srv.req": "Linux x86_64 · systemd",
    "dl.srv.does": "İstəyə bağlıdır. Cihazlarınızı bir-biri ilə tanış edir və onların şifrələnmiş trafikini ötürür ki, istənilən yerdən qoşula biləsiniz.",
    "dl.srv.hint": "Bir sətirlik quraşdırma skripti lazımi faylı yükləyir, SHA-256 cəmini yoxlayır və systemd xidmətini qurur.",
    "dl.srv.btn": "Bir əmrlə quraşdırın",
    "dl.note": "Hələlik uzaqdan idarə olunan kompüter yalnız Windows ola bilər; <a href=\"https://github.com/codewithelvin/remote-desktop-rider/blob/main/docs/MACOS.md\">macOS dəstəyi planlaşdırılır</a>. Yeni buraxılışda macOS və Linux faylları Windows quraşdırıcısından bir neçə dəqiqə sonra görünür.",
    "dl.sums": "Bütün faylların yoxlama cəmləri (SHA256SUMS)",
    "dl.meta.missing": "Bu buraxılışda hələ yoxdur: düymə bütün faylların olduğu buraxılış səhifəsini açır.",
    "dl.meta.offline": "Bütün faylların olduğu buraxılış səhifəsini açır.",

    "faq.title": "Problemlərin həlli",
    "faq1.q": "Windows “Unknown publisher” (naməlum naşir) xəbərdarlığı göstərir",
    "faq1.a": "Quraşdırıcı hələ rəqəmsal imza ilə imzalanmayıb, buna görə SmartScreen xəbərdarlıq edə bilər. <b>More info → Run anyway</b> seçin. Yüklənmiş faylı yoxlamaq üçün onun SHA-256 cəmini buraxılış səhifəsindəki <code>SHA256SUMS</code> faylında həmin fayla aid sətirlə müqayisə edin (PowerShell-də: <code>Get-FileHash rdr-setup-x.y.z.exe</code>).",
    "faq7.q": "macOS tətbiqin açıla bilmədiyini bildirir",
    "faq7.a": "Tətbiq hələ Apple Developer ID ilə imzalanmadığı üçün macOS ilk açılışı bloklayır. <b>System Settings → Privacy &amp; Security</b> bölməsini açın, Remote Desktop Rider barədə bildirişin yanındakı <b>Open Anyway</b> düyməsini klikləyin və təsdiqləyin. Və ya «Applications» qovluğunda tətbiqin üzərində sağ klikləyib <b>Open</b> seçin. Bunu yalnız bir dəfə etmək lazımdır.",
    "faq2.q": "«Cihaz əlavə et» düyməsini kliklədikdə kod görünmür",
    "faq2.a": "Kod serverinizdən gəlir. «Bu kompüter» → «Bu kompüteri idarə et» pəncərəsində «Server» ayarını (ünvan və barmaq izi) təyin edin və yenidən cəhd edin. Server olmadan bu kompüteri yalnız eyni şəbəkədəki cihazlar, pəncərədə göstərilən ünvanlardan biri ilə əlavə edə bilər.",
    "faq3.q": "Korporativ şəbəkədən qoşulmaq alınmır",
    "faq3.a": "Server buna uyğun qurulubsa, problem avtomatik həll olunur. UDP 47800 portu bloklanıbsa, uzaq kompüter təsadüfi yüksək nömrəli portdan istifadə edir. UDP trafiki ümumiyyətlə keçmirsə, hər iki tərəf sistem proksisi vasitəsilə TCP 443 portunda HTTPS tunelinə keçir. Bunun üçün serverdə etibarlı sertifikatı olan nginx lazımdır (yuxarıdakı <code>--domain</code> parametri onu quraşdırır).",
    "faq4.q": "Görüntü bulanıq və ya qırıq-qırıqdır",
    "faq4.a": "Auto rejimində keyfiyyət bağlantıya uyğunlaşır. İstəsəniz, alətlər panelinin keyfiyyət menyusundan sabit ayırdetmə seçin: zəif bağlantıda 720p və ya 480p çox vaxt daha axıcı olur. Başlıq zolağında gediş-qayıdış vaxtı (RTT) və gecikmə göstərilir.",
    "faq5.q": "Win+L uzaq kompüteri deyil, öz kompüterimi kilidləyir",
    "faq5.a": "Windows Win+L (və Ctrl+Alt+Del) kombinasiyalarını lokal kompüter üçün ayırır. Uzaq kompüteri kilidləmək üçün <kbd>Ctrl+Alt+Shift+L</kbd>, Ctrl+Alt+Del üçün isə <kbd>Ctrl+Alt+End</kbd> və ya alətlər panelindən istifadə edin.",
    "faq6.q": "Sürətli qoşulma zamanı administrator pəncərəsini idarə edə bilmirəm",
    "faq6.a": "Sürətli qoşulma administrator hüquqları olmadan işləyir, ona görə administrator kimi işləyən pəncərələri və UAC sorğularını idarə etmək mümkün deyil. Bunun üçün adi, cütləşdirilmiş girişdən istifadə edin.",

    "final.title": "Başlamağa hazırsınız?",
    "final.p": "Proqramı hər iki kompüterə quraşdırın və bir dəqiqədən az müddətdə cütləşdirin.",
    "ft.releases": "Buraxılışlar",
    "sign.link": "Kod imzalama",
    "sign.text": "<b>Sisteminiz niyə xəbərdarlıq edə bilər.</b> Təsdiqlənmiş naşir adının göstərilməsi üçün kod imzalama sertifikatı lazımdır və onun haqqı hər il ödənilir. Remote Desktop Rider açıq mənbə həvəskarları tərəfindən hazırlanır və belə sertifikat almaq üçün büdcəmiz yoxdur. Buna görə hələlik Windows naşiri <b>Unknown</b> (naməlum) kimi göstərəcək, Windows və ya macOS isə tətbiqin quraşdırılmasının təhlükəli ola biləcəyi barədə xəbərdarlıq edə bilər. Hər buraxılış açıq mənbə koddan GitHub Actions vasitəsilə hamının gözü qarşısında yığılır; yüklənmiş faylı yoxlamaq üçün onu <code>SHA256SUMS</code> ilə müqayisə edin.",
    "sign.more": "Ətraflı",
    "ft.privacy": "Məxfilik",
    "ft.license": "MIT lisenziyası",
    "ft.roadmap": "Yol xəritəsi"
  };

  var EN_UI = {
    "copy": "Copy",
    "copy.done": "Copied",
    "copy.fail": "Press Ctrl+C",
    "copy.status": "Command copied to clipboard.",
    "cta.download.windows": "Download for Windows",
    "cta.download.mac": "Download for macOS",
    "cta.download.linux": "Download for Linux",
    "cta.download.any": "Download",
    "dl.meta.missing": "Not on this release yet: the button opens the release page with all files.",
    "dl.meta.offline": "Opens the release page with all files."
  };

  var STORAGE_KEY = "rdr-lang";
  var current = "en";

  var textEls = Array.prototype.slice.call(document.querySelectorAll("[data-i18n]"));
  textEls.forEach(function (el) { el._en = el.innerHTML; });

  var attrEls = Array.prototype.slice.call(document.querySelectorAll("[data-i18n-attr]"));
  attrEls.forEach(function (el) {
    el._enAttr = {};
    el.getAttribute("data-i18n-attr").split(";").forEach(function (pair) {
      var i = pair.indexOf(":");
      if (i < 0) return;
      var attr = pair.slice(0, i).trim();
      el._enAttr[attr] = { key: pair.slice(i + 1).trim(), value: el.getAttribute(attr) };
    });
  });

  function t(key) {
    if (current === "az" && Object.prototype.hasOwnProperty.call(AZ, key)) return AZ[key];
    return EN_UI[key];
  }

  function apply(lang) {
    current = lang === "az" ? "az" : "en";
    var az = current === "az";
    document.documentElement.lang = current;

    textEls.forEach(function (el) {
      var key = el.getAttribute("data-i18n");
      var value = az && Object.prototype.hasOwnProperty.call(AZ, key) ? AZ[key] : el._en;
      if (el.innerHTML !== value) el.innerHTML = value;
    });

    attrEls.forEach(function (el) {
      Object.keys(el._enAttr).forEach(function (attr) {
        var item = el._enAttr[attr];
        var value = az && Object.prototype.hasOwnProperty.call(AZ, item.key) ? AZ[item.key] : item.value;
        el.setAttribute(attr, value);
      });
    });

    document.querySelectorAll(".lang-btn").forEach(function (btn) {
      btn.setAttribute("aria-pressed", btn.getAttribute("data-lang") === current ? "true" : "false");
    });

    renderDownloads();
  }

  var RELEASES_URL = "https://github.com/codewithelvin/remote-desktop-rider/releases/latest";
  var API_URL = "https://api.github.com/repos/codewithelvin/remote-desktop-rider/releases/latest";
  var PATTERNS = {
    windows: /^rdr-setup-.*\.exe$/,
    mac: /-macos\.dmg$/,
    linux: /_amd64\.deb$/,
    sums: /^SHA256SUMS$/
  };
  var release = { state: "loading", version: "", assets: {} };

  function detectOS() {
    var uad = navigator.userAgentData;
    var ua = navigator.userAgent || "";
    var platform = (uad && uad.platform) || navigator.platform || "";
    if ((uad && uad.mobile) || /android|iphone|ipad|ipod/i.test(ua)) return "any";
    if (/mac/i.test(platform) && navigator.maxTouchPoints > 1) return "any";
    if (/win/i.test(platform) || /windows/i.test(ua)) return "windows";
    if (/mac/i.test(platform) || /mac os x/i.test(ua)) return "mac";
    if (/cros/i.test(ua)) return "any";
    if (/linux|x11/i.test(platform) || /linux|x11/i.test(ua)) return "linux";
    return "any";
  }
  var visitorOS = detectOS();

  function formatSize(bytes) {
    var mb = bytes / 1048576;
    try {
      return mb.toLocaleString(current === "az" ? "az" : "en", { minimumFractionDigits: 1, maximumFractionDigits: 1 }) + " MB";
    } catch (e) {
      return mb.toFixed(1) + " MB";
    }
  }

  function assetHref(key) {
    var a = release.assets[key];
    return a ? a.url : RELEASES_URL;
  }

  function renderDownloads() {
    document.querySelectorAll("[data-asset]").forEach(function (el) {
      el.setAttribute("href", assetHref(el.getAttribute("data-asset")));
    });

    document.querySelectorAll("[data-meta]").forEach(function (el) {
      var a = release.assets[el.getAttribute("data-meta")];
      if (a) el.textContent = release.version + " · " + formatSize(a.size);
      else if (release.state === "ok") el.textContent = t("dl.meta.missing");
      else if (release.state === "failed") el.textContent = t("dl.meta.offline");
      else el.textContent = "";
    });

    var apt = document.getElementById("apt-cmd");
    if (apt && release.assets.linux) apt.textContent = "sudo apt install ./" + release.assets.linux.name;

    document.querySelectorAll(".js-dl-primary").forEach(function (btn) {
      btn.setAttribute("href", visitorOS === "any" ? "#download" : assetHref(visitorOS));
      var label = btn.querySelector("[data-dl-label]");
      if (label) label.textContent = t("cta.download." + visitorOS);
    });

    document.querySelectorAll(".dl-card").forEach(function (card) {
      card.classList.toggle("is-yours", card.getAttribute("data-os") === visitorOS);
    });
  }

  if (window.fetch) {
    fetch(API_URL, { headers: { Accept: "application/vnd.github+json" } })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); })
      .then(function (rel) {
        release.version = rel.tag_name || "";
        (rel.assets || []).forEach(function (asset) {
          Object.keys(PATTERNS).forEach(function (key) {
            if (!release.assets[key] && PATTERNS[key].test(asset.name) && asset.browser_download_url) {
              release.assets[key] = { url: asset.browser_download_url, size: asset.size || 0, name: asset.name };
            }
          });
        });
        release.state = "ok";
        renderDownloads();
      })
      .catch(function () {
        release.state = "failed";
        renderDownloads();
      });
  } else {
    release.state = "failed";
  }

  function saved() {
    try { return window.localStorage.getItem(STORAGE_KEY); } catch (e) { return null; }
  }

  function save(lang) {
    try { window.localStorage.setItem(STORAGE_KEY, lang); } catch (e) {}
  }

  var initial = saved();
  if (initial !== "en" && initial !== "az") {
    var nav = (navigator.languages && navigator.languages[0]) || navigator.language || "";
    initial = /^az\b/i.test(nav) ? "az" : "en";
  }
  apply(initial);

  document.querySelectorAll(".lang-btn").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var lang = btn.getAttribute("data-lang");
      apply(lang);
      save(lang);
    });
  });

  var status = document.querySelector(".copy-status");

  function fallbackCopy(text) {
    var ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "absolute";
    ta.style.left = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    var ok = false;
    try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
    document.body.removeChild(ta);
    return ok ? Promise.resolve() : Promise.reject();
  }

  document.querySelectorAll("button.copy[data-copy]").forEach(function (btn) {
    var label = btn.querySelector("span");
    btn.addEventListener("click", function () {
      var src = document.getElementById(btn.getAttribute("data-copy"));
      if (!src) return;
      var text = src.textContent.trim();
      var p = navigator.clipboard && window.isSecureContext
        ? navigator.clipboard.writeText(text).catch(function () { return fallbackCopy(text); })
        : fallbackCopy(text);
      p.then(function () {
        btn.classList.add("done");
        label.textContent = t("copy.done");
        if (status) status.textContent = t("copy.status");
      }, function () {
        label.textContent = t("copy.fail");
        var range = document.createRange();
        range.selectNodeContents(src);
        var sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }).then(function () {
        setTimeout(function () {
          btn.classList.remove("done");
          label.textContent = t("copy");
          if (status) status.textContent = "";
        }, 2000);
      });
    });
  });
})();
