use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "izole")]
#[command(about = "Şeffaf İzolasyonlu Paket Yöneticisi Katmanı", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Yeni bir izole ortam oluşturur
    Create {
        /// Ortamın adı
        env_name: String,
    },
    /// Belirtilen ortama paket(leri) kurar (Ortam yoksa otomatik oluşturulur)
    Install {
        /// Ortamın adı veya kurulacak tek paket
        env_name: String,
        /// Kurulacak diğer paketlerin adları (isteğe bağlı)
        #[arg(required = false)]
        packages: Vec<String>,
    },
    /// Belirtilen ortam içinde komut çalıştırır
    Run {
        /// Ortamın adı
        env_name: String,
        /// Çalıştırılacak komut
        command: String,
        /// Komutun argümanları
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Mevcut izole ortamları listeler
    List,
    /// Belirtilen izole ortamı siler
    Delete {
        /// Ortamın adı
        env_name: String,
    },
    /// Belirtilen izole ortamın içine girer (İnteraktif Kabuk)
    Enter {
        /// Ortamın adı
        env_name: String,
    },
    /// Belirtilen izole ortam hakkında detaylı bilgi gösterir
    Info {
        /// Ortamın adı
        env_name: String,
    },
    /// Belirtilen dosya/binary'yi ortamın usr/bin dizinine kopyalar
    Register {
        /// Ortamın adı
        env_name: String,
        /// Kopyalanacak dosya/binary yolu
        binary_path: String,
    },
    /// Uygulama için doğrudan çalıştırma takma adı (alias) oluşturur
    Alias {
        /// Ortamın adı
        env_name: String,
        /// Ortamdaki binary adı
        binary_name: String,
        /// Takma ad (isteğe bağlı, belirtilmezse binary adı kullanılır)
        alias_name: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Default)]
struct EnvMetadata {
    packages: Vec<String>,
}

fn get_base_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja/.local/share"))
        .join("izole")
}

fn get_envs_dir() -> PathBuf {
    get_base_dir().join("envs")
}

fn get_env_path(env_name: &str) -> PathBuf {
    get_envs_dir().join(env_name)
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(cmd) => match cmd {
            Commands::Create { env_name } => create_env(&env_name),
            Commands::Install { env_name, packages } => {
                let (resolved_env_name, target_packages) = if packages.is_empty() {
                    let extracted = extract_env_name(&env_name);
                    (extracted, vec![env_name.clone()])
                } else {
                    (env_name, packages)
                };
                install_packages(&resolved_env_name, &target_packages)
            }
            Commands::Run { env_name, command, args } => {
                let code = run_env(&env_name, &command, &args)?;
                std::process::exit(code);
            }
            Commands::List => list_envs(),
            Commands::Delete { env_name } => delete_env(&env_name),
            Commands::Enter { env_name } => {
                let code = enter_env(&env_name)?;
                std::process::exit(code);
            }
            Commands::Info { env_name } => info_env(&env_name),
            Commands::Register { env_name, binary_path } => register_binary(&env_name, &binary_path),
            Commands::Alias { env_name, binary_name, alias_name } => alias_binary(&env_name, &binary_name, alias_name.as_deref()),
        },
        None => interactive_menu(),
    }
}

fn create_env(env_name: &str) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    if env_path.exists() {
        eprintln!("Hata: '{}' isimli ortam zaten mevcut.", env_name);
        std::process::exit(1);
    }

    // Klasör yapısını oluştur
    let usr_path = env_path.join("usr");
    fs::create_dir_all(&usr_path)?;
    fs::create_dir_all(usr_path.join("bin"))?;
    fs::create_dir_all(usr_path.join("lib"))?;
    fs::create_dir_all(usr_path.join("lib64"))?;
    fs::create_dir_all(usr_path.join("share"))?;

    // Boş metadata oluştur
    let metadata = EnvMetadata::default();
    let metadata_path = env_path.join("installed.json");
    let file = File::create(metadata_path)?;
    serde_json::to_writer_pretty(file, &metadata)?;

    println!("Başarılı: '{}' ortamı başarıyla oluşturuldu.", env_name);
    println!("Dizin: {}", env_path.display());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Dnf,
    Apt,
    Pacman,
}

fn detect_package_manager() -> PackageManager {
    if Command::new("dnf").arg("--version").output().is_ok() {
        PackageManager::Dnf
    } else if Command::new("apt-get").arg("--version").output().is_ok() {
        PackageManager::Apt
    } else if Command::new("pacman").arg("--version").output().is_ok() {
        PackageManager::Pacman
    } else {
        PackageManager::Dnf
    }
}

fn get_apt_dependencies(packages: &[String]) -> Vec<String> {
    let mut all_pkgs = std::collections::HashSet::new();
    for pkg in packages {
        all_pkgs.insert(pkg.clone());
        let output = Command::new("apt-cache")
            .args(["depends", "--recurse", "--no-recommends", "--no-suggests", "--no-conflicts", "--no-breaks", "--no-replaces", "--no-enhances", pkg])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("Depends:") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let dep = parts[1].trim();
                        if !dep.starts_with('<') && !dep.ends_with('>') {
                            all_pkgs.insert(dep.to_string());
                        }
                    }
                } else if trimmed.starts_with("PreDepends:") {
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let dep = parts[1].trim();
                        if !dep.starts_with('<') && !dep.ends_with('>') {
                            all_pkgs.insert(dep.to_string());
                        }
                    }
                }
            }
        }
    }
    all_pkgs.into_iter().collect()
}

fn get_pacman_urls(packages: &[String]) -> Vec<String> {
    let mut urls = Vec::new();
    let mut cmd = Command::new("pacman");
    cmd.args(["-Sp", "--noconfirm"]);
    for pkg in packages {
        cmd.arg(pkg);
    }
    if let Ok(out) = cmd.output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with("ftp://") || trimmed.starts_with("file://") {
                urls.push(trimmed.to_string());
            }
        }
    }
    urls
}

fn extract_rpm(rpm_path: &Path, target_dir: &Path) -> io::Result<()> {
    let mut rpm2cpio = Command::new("rpm2cpio")
        .arg(rpm_path)
        .stdout(Stdio::piped())
        .spawn()?;

    let stdout = rpm2cpio.stdout.take().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "Failed to capture rpm2cpio stdout")
    })?;

    let mut cpio = Command::new("cpio")
        .args(["-idmv"])
        .current_dir(target_dir)
        .stdin(stdout)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let status = cpio.wait()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("cpio failed with exit status: {}", status),
        ));
    }

    let _ = rpm2cpio.wait()?;
    Ok(())
}

fn extract_deb(deb_path: &Path, target_dir: &Path) -> io::Result<()> {
    let status = Command::new("dpkg")
        .args(["-x", deb_path.to_str().unwrap(), target_dir.to_str().unwrap()])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "dpkg -x failed"));
    }
    Ok(())
}

fn extract_tar_zst(pkg_path: &Path, target_dir: &Path) -> io::Result<()> {
    let status = Command::new("tar")
        .args(["-xf", pkg_path.to_str().unwrap(), "-C", target_dir.to_str().unwrap()])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, "tar -xf failed"));
    }
    Ok(())
}

fn extract_env_name(source: &str) -> String {
    if source.starts_with("http://") || source.starts_with("https://") {
        let mut clean = source.trim_end_matches(".git").to_string();
        while clean.ends_with('/') {
            clean.pop();
        }
        if let Some(last_slash) = clean.rfind('/') {
            let name = &clean[last_slash + 1..];
            return name.to_lowercase();
        }
    }
    let path = Path::new(source);
    if path.exists() && path.is_file() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let mut name = stem.to_lowercase();
            if let Some(dash_idx) = name.find('-') {
                name = name[..dash_idx].to_string();
            }
            return name;
        }
    }
    source.to_string()
}

fn handle_arch_package_install(pkg_name: &str, tmp_downloads: &Path) -> io::Result<bool> {
    let host_arch = std::env::consts::ARCH;
    let arch_name = match host_arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => "x86_64",
    };
    
    println!("[İzole] Arch Linux depolarında paket aranıyor: {} ({})...", pkg_name, arch_name);
    
    let search_url = format!("https://archlinux.org/packages/search/json/?name={}", pkg_name);
    let output = Command::new("curl")
        .args(["-sSL", "-H", "User-Agent: izole", &search_url])
        .output();
        
    if let Ok(out) = output {
        if out.status.success() {
            let json_str = String::from_utf8_lossy(&out.stdout);
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(results) = json_val.get("results").and_then(|r| r.as_array()) {
                    let mut found_pkg = None;
                    for res in results {
                        if let (Some(name), Some(arch), Some(repo)) = (
                            res.get("pkgname").and_then(|n| n.as_str()),
                            res.get("arch").and_then(|a| a.as_str()),
                            res.get("repo").and_then(|r| r.as_str()),
                        ) {
                            if name == pkg_name && arch == arch_name {
                                found_pkg = Some((name.to_string(), arch.to_string(), repo.to_string()));
                                break;
                            }
                        }
                    }
                    
                    if let Some((name, arch, repo)) = found_pkg {
                        let download_url = format!("https://archlinux.org/packages/{}/{}/{}/download/", repo, arch, name);
                        println!("[İzole] Arch paketi bulundu. İndiriliyor: {}...", download_url);
                        
                        let dest_filename = format!("{}.pkg.tar.zst", name);
                        let dest_path = tmp_downloads.join(&dest_filename);
                        
                        let status = Command::new("curl")
                            .args(["-sSL", "-o", dest_path.to_str().unwrap(), &download_url])
                            .status()?;
                            
                        if status.success() {
                            println!("[İzole] Arch paketi indirme tamamlandı: {}", dest_filename);
                            return Ok(true);
                        } else {
                            return Err(io::Error::new(io::ErrorKind::Other, "Arch package download failed"));
                        }
                    } else {
                        println!("Hata: Arch depolarında '{}' adında ve '{}' mimarisinde paket bulunamadı.", pkg_name, arch_name);
                    }
                }
            }
        }
    }
    
    Ok(false)
}

fn handle_custom_install(_env_name: &str, pkg_source: &str, tmp_downloads: &Path) -> io::Result<bool> {
    if pkg_source.starts_with("arch:") {
        let real_pkg_name = &pkg_source[5..];
        return handle_arch_package_install(real_pkg_name, tmp_downloads);
    }

    let pm = detect_package_manager();
    
    if pkg_source.starts_with("http://") || pkg_source.starts_with("https://") {
        if pkg_source.contains("github.com") {
            let mut clean_url = pkg_source.trim_end_matches(".git").to_string();
            while clean_url.ends_with('/') {
                clean_url.pop();
            }
            
            let parts: Vec<&str> = clean_url.split("github.com/").collect();
            if parts.len() >= 2 {
                let repo_path = parts[1];
                let path_parts: Vec<&str> = repo_path.split('/').collect();
                if path_parts.len() >= 2 {
                    let owner = path_parts[0];
                    let repo = path_parts[1];
                    println!("[İzole] GitHub API üzerinden en son sürüm sorgulanıyor: {}/{}...", owner, repo);
                    
                    let api_url = format!("https://api.github.com/repos/{}/{}/releases/latest", owner, repo);
                    let output = Command::new("curl")
                        .args(["-sSL", "-H", "User-Agent: izole", &api_url])
                        .output();
                        
                    if let Ok(out) = output {
                        if out.status.success() {
                            let json_str = String::from_utf8_lossy(&out.stdout);
                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                                if let Some(assets) = json_val.get("assets").and_then(|a| a.as_array()) {
                                    let extension = match pm {
                                        PackageManager::Dnf => ".rpm",
                                        PackageManager::Apt => ".deb",
                                        PackageManager::Pacman => ".pkg.tar.",
                                    };
                                    
                                    let host_arch = std::env::consts::ARCH;
                                    let mut found_asset = None;
                                    let mut best_score = -100;
                                    
                                    for asset in assets {
                                        if let Some(name) = asset.get("name").and_then(|n| n.as_str()) {
                                            let name_lower = name.to_lowercase();
                                            if name_lower.contains(extension) {
                                                let mut score = 0;
                                                if host_arch == "x86_64" {
                                                    if name_lower.contains("x86_64") || name_lower.contains("amd64") || name_lower.contains("x64") {
                                                        score += 10;
                                                    }
                                                    if name_lower.contains("i386") || name_lower.contains("i686") || name_lower.contains("386") || name_lower.contains("arm") || name_lower.contains("aarch64") {
                                                        score -= 20;
                                                    }
                                                } else if host_arch == "aarch64" || host_arch == "arm64" {
                                                    if name_lower.contains("aarch64") || name_lower.contains("arm64") {
                                                        score += 10;
                                                    }
                                                    if name_lower.contains("x86_64") || name_lower.contains("amd64") || name_lower.contains("i386") {
                                                        score -= 20;
                                                    }
                                                }
                                                
                                                if score > best_score {
                                                    best_score = score;
                                                    found_asset = Some(asset.clone());
                                                }
                                            }
                                        }
                                    }
                                    
                                    if let Some(asset) = found_asset {
                                        let asset_name = asset.get("name").and_then(|n| n.as_str()).unwrap();
                                        let download_url = asset.get("browser_download_url").and_then(|u| u.as_str()).unwrap();
                                        println!("[İzole] Uyumlu paket bulundu: {}...", asset_name);
                                        println!("[İzole] İndiriliyor: {}...", download_url);
                                        
                                        let status = Command::new("curl")
                                            .args(["-sSL", "-O", download_url])
                                            .current_dir(tmp_downloads)
                                            .status()?;
                                            
                                        if status.success() {
                                            println!("[İzole] İndirme tamamlandı: {}", asset_name);
                                            return Ok(true);
                                        } else {
                                            return Err(io::Error::new(io::ErrorKind::Other, "GitHub release asset download failed"));
                                        }
                                    } else {
                                        println!("Uyarı: Bu GitHub sürümünde işletim sisteminizle uyumlu bir paket ({} uzantılı) bulunamadı.", extension);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        let extension = if pkg_source.contains(".rpm") {
            Some("rpm")
        } else if pkg_source.contains(".deb") {
            Some("deb")
        } else if pkg_source.contains(".pkg.tar.") || pkg_source.contains(".tar.zst") || pkg_source.contains(".tar.xz") {
            Some("pkg")
        } else {
            None
        };
        
        if let Some(_ext) = extension {
            println!("[İzole] Doğrudan URL üzerinden indiriliyor: {}...", pkg_source);
            let status = Command::new("curl")
                .args(["-sSL", "-O", pkg_source])
                .current_dir(tmp_downloads)
                .status()?;
            if status.success() {
                return Ok(true);
            }
        }
    }
    
    let path = Path::new(pkg_source);
    if path.exists() && path.is_file() {
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extension == "rpm" || extension == "deb" || extension == "zst" || extension == "xz" || pkg_source.contains(".pkg.tar.") {
            println!("[İzole] Yerel dosya kopyalanıyor: {}...", pkg_source);
            let dest_file = tmp_downloads.join(path.file_name().unwrap());
            fs::copy(path, dest_file)?;
            return Ok(true);
        }
    }
    
    Ok(false)
}

fn install_packages(env_name: &str, packages: &[String]) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    
    if !env_path.exists() {
        println!("Ortam '{}' mevcut değil. Otomatik olarak oluşturuluyor...", env_name);
        create_env(env_name)?;
    }

    println!("Ortam: {}", env_name);
    println!("Kurulacak paketler/kaynaklar: {:?}", packages);

    let tmp_downloads = env_path.join("tmp_downloads");
    if tmp_downloads.exists() {
        fs::remove_dir_all(&tmp_downloads)?;
    }
    fs::create_dir_all(&tmp_downloads)?;

    let mut system_packages = Vec::new();
    let mut custom_installed_count = 0;

    for pkg in packages {
        match handle_custom_install(env_name, pkg, &tmp_downloads) {
            Ok(true) => {
                custom_installed_count += 1;
            }
            Ok(false) => {
                system_packages.push(pkg.clone());
            }
            Err(e) => {
                eprintln!("Uyarı: Özel kaynak yüklenirken hata oluştu ({pkg}): {e}");
            }
        }
    }

    if !system_packages.is_empty() {
        let pm = detect_package_manager();
        match pm {
            PackageManager::Dnf => {
                println!("Paketler indiriliyor (dnf download)...");
                let mut dnf_cmd = Command::new("dnf");
                dnf_cmd
                    .arg("download")
                    .arg("-y")
                    .arg("--resolve")
                    .arg(format!("--destdir={}", tmp_downloads.display()));

                for pkg in &system_packages {
                    dnf_cmd.arg(pkg);
                }

                let status = dnf_cmd.status()?;
                if !status.success() {
                    eprintln!("Hata: dnf download başarısız oldu.");
                    fs::remove_dir_all(&tmp_downloads)?;
                    std::process::exit(1);
                }
            }
            PackageManager::Apt => {
                println!("Bağımlılıklar sorgulanıyor (apt-cache)...");
                let all_packages = get_apt_dependencies(&system_packages);
                println!("Paketler ve bağımlılıkları indiriliyor (apt-get download)...");
                
                let status = Command::new("apt-get")
                    .arg("download")
                    .args(&all_packages)
                    .current_dir(&tmp_downloads)
                    .status()?;

                if !status.success() {
                    eprintln!("Hata: apt-get download başarısız oldu.");
                    fs::remove_dir_all(&tmp_downloads)?;
                    std::process::exit(1);
                }
            }
            PackageManager::Pacman => {
                println!("Paket indirme URL'leri sorgulanıyor (pacman -Sp)...");
                let urls = get_pacman_urls(&system_packages);
                if urls.is_empty() {
                    eprintln!("Hata: pacman indirme URL'leri alınamadı.");
                    fs::remove_dir_all(&tmp_downloads)?;
                    std::process::exit(1);
                }

                println!("Paketler indiriliyor (curl)...");
                for url in &urls {
                    println!("İndiriliyor: {}", url);
                    let status = Command::new("curl")
                        .args(["-sSL", "-O", url])
                        .current_dir(&tmp_downloads)
                        .status()?;
                    if !status.success() {
                        eprintln!("Hata: '{}' indirilemedi.", url);
                        fs::remove_dir_all(&tmp_downloads)?;
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    println!("Paketler arşivden çıkarılıyor...");
    let mut extracted_any = false;
    for entry in fs::read_dir(&tmp_downloads)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let filename = path.file_name().unwrap().to_string_lossy().to_lowercase();
            if filename.ends_with(".rpm") {
                println!("Açılıyor (RPM): {}", path.file_name().unwrap().to_string_lossy());
                extract_rpm(&path, &env_path)?;
                extracted_any = true;
            } else if filename.ends_with(".deb") {
                println!("Açılıyor (DEB): {}", path.file_name().unwrap().to_string_lossy());
                extract_deb(&path, &env_path)?;
                extracted_any = true;
            } else if filename.contains(".pkg.tar.") || filename.ends_with(".tar.zst") || filename.ends_with(".tar.xz") || filename.ends_with(".tar.gz") {
                println!("Açılıyor (Arch/Tarball): {}", path.file_name().unwrap().to_string_lossy());
                extract_tar_zst(&path, &env_path)?;
                extracted_any = true;
            }
        }
    }

    if !extracted_any && custom_installed_count == 0 {
        println!("Bilgi: İndirilecek veya kurulacak yeni paket bulunamadı.");
        fs::remove_dir_all(&tmp_downloads)?;
        return Ok(());
    }

    println!("Masaüstü entegrasyonu kontrol ediliyor...");
    if let Err(e) = scan_and_generate_desktop_entries(env_name) {
        eprintln!("Uyarı: Masaüstü kısayolları oluşturulamadı: {}", e);
    }

    let metadata_path = env_path.join("installed.json");
    let mut metadata = if metadata_path.exists() {
        let file = File::open(&metadata_path)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).unwrap_or_default()
    } else {
        EnvMetadata::default()
    };

    for pkg in packages {
        if !metadata.packages.contains(pkg) {
            metadata.packages.push(pkg.clone());
        }
    }

    let file = File::create(metadata_path)?;
    serde_json::to_writer_pretty(file, &metadata)?;

    // Otomatik takma ad (alias) oluşturma kontrolü
    let bin_dir = env_path.join("usr/bin");
    if bin_dir.exists() && bin_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&bin_dir) {
            let mut execs = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_file() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt;
                            if let Ok(meta) = path.metadata() {
                                if meta.mode() & 0o111 != 0 {
                                    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                                        execs.push(filename.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !execs.is_empty() {
                let mut target_bin = None;
                if execs.contains(&env_name.to_string()) {
                    target_bin = Some(env_name.to_string());
                } else if execs.len() == 1 {
                    target_bin = Some(execs[0].clone());
                }

                if let Some(bin_name) = target_bin {
                    println!("\n[İzole] Otomatik terminal takma adı (alias) oluşturuluyor: {}...", env_name);
                    let _ = alias_binary(env_name, &bin_name, Some(env_name));
                }
            }
        }
    }

    fs::remove_dir_all(&tmp_downloads)?;
    println!("Kurulum tamamlandı!");
    Ok(())
}

fn run_env(env_name: &str, command: &str, args: &[String]) -> io::Result<i32> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' ortamı mevcut değil.", env_name);
        std::process::exit(1);
    }

    // NVIDIA Sürücülerini kontrol et ve otomatik hizala
    if let Err(e) = align_nvidia_drivers_if_needed(env_name) {
        eprintln!("Grafik sürücüleri kontrol edilirken hata oluştu: {}", e);
    }

    let mut retry_count = 0;
    let max_retries = 3;

    loop {
        // Temel bwrap argümanları
        let mut bwrap_args = vec![
            "--dev-bind".to_string(), "/dev".to_string(), "/dev".to_string(),
            "--bind".to_string(), "/run".to_string(), "/run".to_string(),
            "--bind".to_string(), "/tmp".to_string(), "/tmp".to_string(),
            "--proc".to_string(), "/proc".to_string(),
            "--bind".to_string(), "/sys".to_string(), "/sys".to_string(),
            "--bind".to_string(), "/home".to_string(), "/home".to_string(),
            "--bind".to_string(), "/var".to_string(), "/var".to_string(),
            "--setenv".to_string(), "IZOLE_ENV".to_string(), env_name.to_string(),
        ];

        // /etc overlay kontrolü
        let env_etc = env_path.join("etc");
        if env_etc.exists() && env_etc.is_dir() {
            bwrap_args.push("--overlay-src".to_string());
            bwrap_args.push("/etc".to_string());
            bwrap_args.push("--overlay-src".to_string());
            bwrap_args.push(env_etc.to_str().unwrap().to_string());
            bwrap_args.push("--ro-overlay".to_string());
            bwrap_args.push("/etc".to_string());
        } else {
            bwrap_args.push("--bind".to_string());
            bwrap_args.push("/etc".to_string());
            bwrap_args.push("/etc".to_string());
        }

        // /usr overlay (Her zaman gerekli)
        let env_usr = env_path.join("usr");
        bwrap_args.push("--overlay-src".to_string());
        bwrap_args.push("/usr".to_string());
        bwrap_args.push("--overlay-src".to_string());
        bwrap_args.push(env_usr.to_str().unwrap().to_string());
        bwrap_args.push("--ro-overlay".to_string());
        bwrap_args.push("/usr".to_string());

        // Temel sistem sembolik bağları
        bwrap_args.push("--symlink".to_string());
        bwrap_args.push("usr/lib".to_string());
        bwrap_args.push("/lib".to_string());

        bwrap_args.push("--symlink".to_string());
        bwrap_args.push("usr/lib64".to_string());
        bwrap_args.push("/lib64".to_string());

        bwrap_args.push("--symlink".to_string());
        bwrap_args.push("usr/bin".to_string());
        bwrap_args.push("/bin".to_string());

        bwrap_args.push("--symlink".to_string());
        bwrap_args.push("usr/sbin".to_string());
        bwrap_args.push("/sbin".to_string());

        // /opt overlay kontrolü
        let env_opt = env_path.join("opt");
        let host_opt = Path::new("/opt");
        if env_opt.exists() && env_opt.is_dir() && host_opt.exists() {
            bwrap_args.push("--overlay-src".to_string());
            bwrap_args.push("/opt".to_string());
            bwrap_args.push("--overlay-src".to_string());
            bwrap_args.push(env_opt.to_str().unwrap().to_string());
            bwrap_args.push("--ro-overlay".to_string());
            bwrap_args.push("/opt".to_string());
        } else if host_opt.exists() {
            bwrap_args.push("--bind".to_string());
            bwrap_args.push("/opt".to_string());
            bwrap_args.push("/opt".to_string());
        }

        // Komut ve argümanlar
        bwrap_args.push("--".to_string());
        bwrap_args.push(command.to_string());
        for arg in args {
            bwrap_args.push(arg.clone());
        }

        use std::os::unix::process::CommandExt;

        // Ana süreçte Ctrl+C (SIGINT) ve sonlandırma sinyallerini yoksay.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }

        let mut cmd = Command::new("bwrap");
        cmd.args(&bwrap_args);

        // Alt süreçte sinyal yöneticilerini varsayılana döndür
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                Ok(())
            });
        }

        // Stderr'i yakala ve gerçek zamanlı olarak konsola yazdır
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let mut stderr_handle = child.stderr.take().unwrap();
        
        let thread_handle = std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = [0; 512];
            let mut captured = Vec::new();
            let mut stderr_writer = std::io::stderr();
            while let Ok(n) = stderr_handle.read(&mut buffer) {
                if n == 0 { break; }
                let _ = stderr_writer.write_all(&buffer[..n]);
                let _ = stderr_writer.flush();
                captured.extend_from_slice(&buffer[..n]);
            }
            String::from_utf8_lossy(&captured).into_owned()
        });

        let status = child.wait()?;

        // Sinyal yöneticilerini ana süreç için varsayılana döndür
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
        }

        let exit_code = status.code().unwrap_or_else(|| {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map(|sig| 128 + sig).unwrap_or(1)
        });

        let stderr_output = thread_handle.join().unwrap_or_default();

        if exit_code != 0 {
            // Hata çıktisinde eksik kütüphane tespiti yap
            if let Some(lib_name) = parse_missing_library(&stderr_output) {
                println!("\n\x1b[1;33m[İzole Self-Healing] Eksik kütüphane tespit edildi: {}\x1b[0m", lib_name);
                if retry_count >= max_retries {
                    println!("\x1b[1;31m[İzole Self-Healing] Maksimum yeniden deneme sınırına ulaşıldı. Durduruluyor.\x1b[0m");
                    return Ok(exit_code);
                }

                let pm_name = match detect_package_manager() {
                    PackageManager::Dnf => "DNF",
                    PackageManager::Apt => "APT",
                    PackageManager::Pacman => "Pacman",
                };
                println!("\x1b[1;34m[İzole Self-Healing] {} üzerinden '{}' kütüphanesini sağlayan paket aranıyor...\x1b[0m", pm_name, lib_name);
                let is_steam = command.contains("steam") || args.iter().any(|arg| arg.contains("steam"));
                if let Some(pkg_name) = find_package_for_library(&lib_name, is_steam) {
                    println!("\x1b[1;32m[İzole Self-Healing] Paket bulundu: '{}'. Otomatik olarak kuruluyor...\x1b[0m", pkg_name);
                    install_packages(env_name, &[pkg_name])?;
                    retry_count += 1;
                    println!("\x1b[1;32m[İzole Self-Healing] Yeniden çalıştırılıyor (Deneme {}/{})...\x1b[0m\n", retry_count, max_retries);
                    continue;
                } else {
                    println!("\x1b[1;31m[İzole Self-Healing] Kütüphaneyi sağlayan paket bulunamadı.\x1b[0m");
                }
            }

            // Genel hata durumunda Ollama ile teşhis yapmayı dene
            if let Some(diag) = run_ollama_diagnosis(command, &stderr_output) {
                println!("\n\x1b[1;36m==================================================\x1b[0m");
                println!("\x1b[1;32m       🤖 YAPAY ZEKA (OLLAMA) HATA TEŞHİSİ        \x1b[0m");
                println!("\x1b[1;36m==================================================\x1b[0m");
                println!("Teşhis: {}\n", diag.explanation);

                if retry_count < max_retries {
                    if diag.fix_action == "install_container_package" && diag.package_name.is_some() {
                        let pkg = diag.package_name.unwrap();
                        let confirm = Confirm::with_theme(&ColorfulTheme::default())
                            .with_prompt(format!("Yapay zeka, izole ortam içine '{}' paketini kurarak çözmeyi öneriyor. Kurulsun mu?", pkg))
                            .default(true)
                            .interact()
                            .unwrap_or(false);

                        if confirm {
                            println!("\x1b[1;34m[İzole AI-Fix] Paket kuruluyor: {}...\x1b[0m", pkg);
                            if install_packages(env_name, &[pkg]).is_ok() {
                                retry_count += 1;
                                println!("\x1b[1;32m[İzole AI-Fix] Uygulama yeniden başlatılıyor (Deneme {}/{})...\x1b[0m\n", retry_count, max_retries);
                                continue;
                            }
                        }
                    } else if diag.fix_action == "run_command" && diag.command_to_run.is_some() {
                        let cmd_str = diag.command_to_run.unwrap();
                        let confirm = Confirm::with_theme(&ColorfulTheme::default())
                            .with_prompt(format!("Yapay zeka, şu komutu çalıştırmayı öneriyor:\n  -> {}\nKomut çalıştırılsın mı?", cmd_str))
                            .default(false)
                            .interact()
                            .unwrap_or(false);

                        if confirm {
                            println!("\x1b[1;34m[İzole AI-Fix] Komut çalıştırılıyor...\x1b[0m");
                            let status = Command::new("bash")
                                .args(["-c", &cmd_str])
                                .status();
                            if status.map(|s| s.success()).unwrap_or(false) {
                                retry_count += 1;
                                println!("\x1b[1;32m[İzole AI-Fix] Uygulama yeniden başlatılıyor (Deneme {}/{})...\x1b[0m\n", retry_count, max_retries);
                                continue;
                            } else {
                                println!("\x1b[1;31m[İzole AI-Fix] Komut başarısız oldu.\x1b[0m");
                            }
                        }
                    }
                }
            }
        }

        return Ok(exit_code);
    }
}

fn scan_and_generate_desktop_entries(env_name: &str) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    let apps_dir = env_path.join("usr/share/applications");
    if apps_dir.exists() && apps_dir.is_dir() {
        for entry in fs::read_dir(apps_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().map_or(false, |ext| ext == "desktop") {
                println!("Masaüstü kısayolu bulundu: {}", path.file_name().unwrap().to_string_lossy());
                if let Err(e) = process_desktop_file(env_name, &path) {
                    eprintln!("Uyarı: Kısayol işlenirken hata oluştu ({}): {}", path.display(), e);
                }
            }
        }
    }
    Ok(())
}

fn process_desktop_file(env_name: &str, file_path: &Path) -> io::Result<()> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut has_exec = false;
    let mut icon_name = None;
    let mut icon_line_index = None;

    for line_result in reader.lines() {
        let line = line_result?;
        lines.push(line);
    }

    for (i, line) in lines.iter_mut().enumerate() {
        if line.starts_with("Exec=") {
            let val = &line["Exec=".len()..];
            let parts: Vec<&str> = val.split_whitespace().collect();
            if !parts.is_empty() {
                let exec_path = Path::new(parts[0]);
                let binary_name = exec_path.file_name().unwrap().to_string_lossy();
                let remaining = parts[1..].join(" ");
                // Kısayolun 'izole' aracılığıyla çalışmasını sağla
                let new_exec = format!("Exec=izole run {} {} {}", env_name, binary_name, remaining);
                *line = new_exec;
                has_exec = true;
            }
        } else if line.starts_with("Icon=") {
            let val = line["Icon=".len()..].trim().to_string();
            icon_name = Some(val);
            icon_line_index = Some(i);
        }
    }

    if !has_exec {
        return Ok(());
    }

    // İkon yolunu çöz
    if let (Some(name), Some(idx)) = (icon_name, icon_line_index) {
        if !name.starts_with('/') {
            if let Some(abs_icon_path) = find_icon_in_env(env_name, &name) {
                lines[idx] = format!("Icon={}", abs_icon_path.to_string_lossy());
            }
        }
    }

    // ~/.local/share/applications dizinini oluştur
    let app_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja/.local/share"))
        .join("applications");
    fs::create_dir_all(&app_dir)?;

    let original_filename = file_path.file_name().unwrap().to_string_lossy();
    let new_filename = format!("izole-{}-{}", env_name, original_filename);
    let new_path = app_dir.join(new_filename);

    let mut out_file = File::create(&new_path)?;
    for line in &lines {
        writeln!(out_file, "{}", line)?;
    }

    println!("Masaüstü entegrasyonu oluşturuldu: {}", new_path.display());
    Ok(())
}

fn find_icon_in_env(env_name: &str, icon_name: &str) -> Option<PathBuf> {
    let env_path = get_env_path(env_name);

    // 1. Standart yollarda tam eşleşme ara
    let search_paths = vec![
        env_path.join("usr/share/icons"),
        env_path.join("usr/share/pixmaps"),
    ];

    for path in &search_paths {
        if path.exists() {
            if let Some(found) = find_icon_recursive(path, icon_name) {
                return Some(found);
            }
        }
    }

    // 2. Tüm ortam klasöründe tam eşleşme ara (örneğin opt/ altında olabilir)
    if let Some(found) = find_icon_recursive(&env_path, icon_name) {
        return Some(found);
    }

    // 3. Fallback: "product_logo" içeren resimleri ara (Chrome gibi uygulamalar için)
    if let Some(found) = find_icon_by_pattern_recursive(&env_path, "product_logo") {
        return Some(found);
    }

    // 4. Fallback: "logo" veya "icon" içeren resimleri ara
    if let Some(found) = find_icon_by_pattern_recursive(&env_path, "logo") {
        return Some(found);
    }
    if let Some(found) = find_icon_by_pattern_recursive(&env_path, "icon") {
        return Some(found);
    }

    None
}

fn find_icon_recursive(dir: &Path, icon_name: &str) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map_or(false, |n| n == "tmp_downloads") {
                        continue;
                    }
                    if let Some(found) = find_icon_recursive(&path, icon_name) {
                        return Some(found);
                    }
                } else {
                    let file_stem = path.file_stem().and_then(|s| s.to_str());
                    let file_ext = path.extension().and_then(|s| s.to_str());
                    if let (Some(stem), Some(ext)) = (file_stem, file_ext) {
                        if stem == icon_name && (ext == "png" || ext == "svg" || ext == "xpm") {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

fn find_icon_by_pattern_recursive(dir: &Path, pattern: &str) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map_or(false, |n| n == "tmp_downloads") {
                        continue;
                    }
                    if let Some(found) = find_icon_by_pattern_recursive(&path, pattern) {
                        return Some(found);
                    }
                } else {
                    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    let file_ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if file_name.to_lowercase().contains(pattern) && (file_ext == "png" || file_ext == "svg" || file_ext == "xpm") {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

fn list_envs() -> io::Result<()> {
    let envs_dir = get_envs_dir();
    if !envs_dir.exists() {
        println!("Bilgi: Henüz hiçbir izole ortam oluşturulmamış.");
        return Ok(());
    }

    println!("{:<20} | {:<40}", "Ortam Adı", "Kurulu Paketler");
    println!("{}", "-".repeat(65));

    for entry in fs::read_dir(envs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let env_name = path.file_name().unwrap().to_string_lossy();
            let metadata_path = path.join("installed.json");
            let mut packages = Vec::new();

            if metadata_path.exists() {
                if let Ok(file) = File::open(metadata_path) {
                    let reader = BufReader::new(file);
                    if let Ok(meta) = serde_json::from_reader::<_, EnvMetadata>(reader) {
                        packages = meta.packages;
                    }
                }
            }

            let pkgs_str = if packages.is_empty() {
                "yok".to_string()
            } else {
                packages.join(", ")
            };

            println!("{:<20} | {:<40}", env_name, pkgs_str);
        }
    }
    Ok(())
}

fn delete_env(env_name: &str) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' isimli ortam mevcut değil.", env_name);
        std::process::exit(1);
    }

    // Ortam dizinini sil
    fs::remove_dir_all(&env_path)?;

    // Bu ortama ait masaüstü kısayollarını temizle
    let app_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja/.local/share"))
        .join("applications");
    if app_dir.exists() {
        if let Ok(entries) = fs::read_dir(app_dir) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_file() {
                        let filename = path.file_name().unwrap().to_string_lossy();
                        if filename.starts_with(&format!("izole-{}-", env_name)) && filename.ends_with(".desktop") {
                            let _ = fs::remove_file(&path);
                            println!("Temizlendi: Masaüstü kısayolu kaldırıldı -> {}", filename);
                        }
                    }
                }
            }
        }
    }

    // Bu ortama ait takma adları temizle
    if let Err(e) = cleanup_aliases(env_name) {
        eprintln!("Uyarı: Takma adlar temizlenirken hata oluştu: {}", e);
    }

    println!("Başarılı: '{}' ortamı silindi.", env_name);
    Ok(())
}

fn enter_env(env_name: &str) -> io::Result<i32> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' ortamı mevcut değil.", env_name);
        std::process::exit(1);
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    println!("İzole ortam '{}' içine giriliyor...", env_name);
    println!("Kabuk: {}", shell);
    println!("Çıkış yapmak için 'exit' yazın veya Ctrl+D basın.");

    run_env(env_name, &shell, &[])
}

fn info_env(env_name: &str) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' isimli ortam mevcut değil.", env_name);
        std::process::exit(1);
    }

    println!("Ortam Bilgileri: {}", env_name);
    println!("{}", "=".repeat(40));
    println!("Dizin: {}", env_path.display());

    let size = get_dir_size(&env_path).unwrap_or(0);
    println!("Disk Boyutu: {}", format_size(size));

    let metadata_path = env_path.join("installed.json");
    let mut packages = Vec::new();
    if metadata_path.exists() {
        if let Ok(file) = File::open(metadata_path) {
            let reader = BufReader::new(file);
            if let Ok(meta) = serde_json::from_reader::<_, EnvMetadata>(reader) {
                packages = meta.packages;
            }
        }
    }

    let pkgs_str = if packages.is_empty() {
        "yok".to_string()
    } else {
        packages.join(", ")
    };
    println!("Kurulu Paketler: {}", pkgs_str);

    let app_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja/.local/share"))
        .join("applications");
    let mut shortcuts = Vec::new();
    if app_dir.exists() {
        if let Ok(entries) = fs::read_dir(app_dir) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    if path.is_file() {
                        let filename = path.file_name().unwrap().to_string_lossy();
                        if filename.starts_with(&format!("izole-{}-", env_name)) && filename.ends_with(".desktop") {
                            shortcuts.push(filename.to_string());
                        }
                    }
                }
            }
        }
    }

    let shortcuts_str = if shortcuts.is_empty() {
        "yok".to_string()
    } else {
        shortcuts.join(", ")
    };
    println!("Masaüstü Kısayolları: {}", shortcuts_str);

    Ok(())
}

fn get_dir_size(path: &Path) -> io::Result<u64> {
    let mut size = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                size += get_dir_size(&entry.path())?;
            } else {
                size += metadata.len();
            }
        }
    } else {
        size += path.metadata()?.len();
    }
    Ok(size)
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn register_binary(env_name: &str, binary_path: &str) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' ortamı mevcut değil.", env_name);
        std::process::exit(1);
    }

    let src_path = Path::new(binary_path);
    if !src_path.exists() {
        eprintln!("Hata: Belirtilen dosya mevcut değil: {}", binary_path);
        std::process::exit(1);
    }

    let filename = src_path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Geçersiz dosya yolu")
    })?;

    let dest_dir = env_path.join("usr/bin");
    fs::create_dir_all(&dest_dir)?;
    let dest_path = dest_dir.join(filename);

    fs::copy(src_path, &dest_path)?;

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&dest_path, fs::Permissions::from_mode(0o755))?;

    println!("Başarılı: '{}' dosyası '{}' ortamının usr/bin dizinine kaydedildi.", filename.to_string_lossy(), env_name);
    Ok(())
}

fn alias_binary(env_name: &str, binary_name: &str, alias_name: Option<&str>) -> io::Result<()> {
    let env_path = get_env_path(env_name);
    if !env_path.exists() {
        eprintln!("Hata: '{}' ortamı mevcut değil.", env_name);
        std::process::exit(1);
    }

    let binary_in_env = env_path.join("usr/bin").join(binary_name);
    if !binary_in_env.exists() {
        println!("Uyarı: Ortamın usr/bin dizininde '{}' dosyası bulunamadı. Yine de takma ad oluşturuluyor...", binary_name);
    }

    let target_alias = alias_name.unwrap_or(binary_name);
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja"))
        .join(".local/bin");
    fs::create_dir_all(&bin_dir)?;
    let alias_path = bin_dir.join(target_alias);

    let izole_exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("/home/ninja/.local/bin/izole"));

    let content = format!(
        "#!/bin/bash\nexec {} run {} {} \"$@\"\n",
        izole_exe.to_string_lossy(),
        env_name,
        binary_name
    );

    let mut file = File::create(&alias_path)?;
    file.write_all(content.as_bytes())?;

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&alias_path, fs::Permissions::from_mode(0o755))?;

    println!("Başarılı: '{}' takma adı oluşturuldu -> {}", target_alias, alias_path.display());
    println!("Terminalinizde doğrudan '{}' yazarak çalıştırabilirsiniz.", target_alias);
    Ok(())
}

fn cleanup_aliases(env_name: &str) -> io::Result<()> {
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/home/ninja"))
        .join(".local/bin");
    if bin_dir.exists() && bin_dir.is_dir() {
        for entry in fs::read_dir(bin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(&path) {
                    let pattern = format!("run {} ", env_name);
                    if content.contains("#!/bin/bash") && content.contains(&pattern) {
                        fs::remove_file(&path)?;
                        println!("Temizlendi: Takma ad kaldırıldı -> {}", path.file_name().unwrap().to_string_lossy());
                    }
                }
            }
        }
    }
    Ok(())
}

fn interactive_menu() -> io::Result<()> {
    let theme = ColorfulTheme::default();
    loop {
        println!("\n\x1b[1;36m==================================================\x1b[0m");
        println!("\x1b[1;32m       🚀 İZOLE - İNTERAKTİF YÖNETİM PANELİ       \x1b[0m");
        println!("\x1b[1;36m==================================================\x1b[0m");

        let options = &[
            "📂 Ortamları Listele",
            "✨ Yeni Ortam Oluştur",
            "📥 Paket Kur",
            "⚡ Uygulama Çalıştır (Run)",
            "🐚 İnteraktif Kabuk Başlat (Enter)",
            "📊 Ortam Bilgisi Görüntüle (Info)",
            "🔗 Takma Ad (Alias) Oluştur",
            "💾 Yerel Dosya/Binary Kaydet (Register)",
            "❌ Ortamı Sil",
            "🚪 Çıkış",
        ];

        let selection = Select::with_theme(&theme)
            .with_prompt("Bir işlem seçin")
            .default(0)
            .items(options)
            .interact_opt()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let index = match selection {
            Some(i) => i,
            None => break, // Ctrl+C veya Escape tuşuyla çıkış
        };

        match index {
            0 => {
                println!("\n\x1b[1;34m--- Mevcut Ortamlar ---\x1b[0m");
                list_envs()?;
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            1 => {
                let name: String = Input::with_theme(&theme)
                    .with_prompt("Yeni ortamın adı")
                    .interact_text()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    println!("\x1b[1;31mHata: Geçersiz ortam adı.\x1b[0m");
                } else {
                    create_env(trimmed)?;
                }
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            2 => {
                let envs = get_envs_list()?;
                let env_name = if envs.is_empty() {
                    let name: String = Input::with_theme(&theme)
                        .with_prompt("Ortam adı (Mevcut ortam yok, yeni oluşturulacak)")
                        .interact_text()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    name.trim().to_string()
                } else {
                    let mut choices = envs.clone();
                    choices.push("[Yeni Ortam Oluştur]".to_string());
                    let sel = Select::with_theme(&theme)
                        .with_prompt("Kurulum yapılacak ortamı seçin")
                        .default(0)
                        .items(&choices)
                        .interact_opt()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    
                    match sel {
                        Some(s) => {
                            if s == envs.len() {
                                let name: String = Input::with_theme(&theme)
                                    .with_prompt("Yeni ortamın adı")
                                    .interact_text()
                                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                                name.trim().to_string()
                            } else {
                                envs[s].clone()
                            }
                        }
                        None => {
                            continue;
                        }
                    }
                };

                if env_name.is_empty() {
                    continue;
                }

                let pkgs_input: String = Input::with_theme(&theme)
                    .with_prompt("Kurulacak paket ad(lar)ı (boşlukla ayırın)")
                    .interact_text()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let pkgs: Vec<String> = pkgs_input
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                if pkgs.is_empty() {
                    println!("\x1b[1;31mHata: Hiç paket belirtilmedi.\x1b[0m");
                } else {
                    install_packages(&env_name, &pkgs)?;
                }
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            3 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı. Önce bir ortam oluşturmalısınız.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Hangi ortamda çalıştıracaksınız?")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                let binaries = get_env_binaries(env_name)?;
                let command = if binaries.is_empty() {
                    let cmd_input: String = Input::with_theme(&theme)
                        .with_prompt("Çalıştırılacak komut (örn: bash)")
                        .interact_text()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    cmd_input
                } else {
                    let mut choices = binaries.clone();
                    choices.push("[Özel Komut Yaz]".to_string());
                    let cmd_sel = Select::with_theme(&theme)
                        .with_prompt("Çalıştırmak istediğiniz uygulamayı seçin")
                        .default(0)
                        .items(&choices)
                        .interact_opt()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    
                    match cmd_sel {
                        Some(cs) => {
                            if cs == binaries.len() {
                                let cmd_input: String = Input::with_theme(&theme)
                                    .with_prompt("Çalıştırılacak komut")
                                    .interact_text()
                                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                                cmd_input
                            } else {
                                binaries[cs].clone()
                            }
                        }
                        None => continue,
                    }
                };

                if command.trim().is_empty() {
                    continue;
                }

                let args_input: String = Input::with_theme(&theme)
                    .with_prompt("Komut argümanları (isteğe bağlı, boşlukla ayırın)")
                    .allow_empty(true)
                    .interact_text()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let args: Vec<String> = args_input
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();

                println!("\x1b[1;32mÇalıştırılıyor: {} {:?}\x1b[0m\n", command, args);
                if let Err(e) = run_env(env_name, command.trim(), &args) {
                    println!("\x1b[1;31mÇalıştırma sırasında hata oluştu: {}\x1b[0m", e);
                }
                println!("\n\x1b[33mUygulama sonlandı. Devam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            4 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Hangi ortama girmek istersiniz?")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                if let Err(e) = enter_env(env_name) {
                    println!("\x1b[1;31mOrtama girilirken hata oluştu: {}\x1b[0m", e);
                }
                println!("\n\x1b[33mKabuk sonlandı. Devam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            5 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Bilgi almak istediğiniz ortamı seçin")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                info_env(env_name)?;
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            6 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Ortamı seçin")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                let binaries = get_env_binaries(env_name)?;
                let binary_name = if binaries.is_empty() {
                    let name: String = Input::with_theme(&theme)
                        .with_prompt("Takma ad oluşturulacak binary adı")
                        .interact_text()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    name
                } else {
                    let sel_bin = Select::with_theme(&theme)
                        .with_prompt("Binary seçin")
                        .default(0)
                        .items(&binaries)
                        .interact_opt()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    
                    match sel_bin {
                        Some(sb) => binaries[sb].clone(),
                        None => continue,
                    }
                };

                if binary_name.trim().is_empty() {
                    continue;
                }

                let alias_name: String = Input::with_theme(&theme)
                    .with_prompt(format!("Takma ad (boş bırakılırsa '{}' kullanılacak)", binary_name))
                    .allow_empty(true)
                    .interact_text()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let alias_opt = if alias_name.trim().is_empty() {
                    None
                } else {
                    Some(alias_name.trim())
                };

                alias_binary(env_name, &binary_name, alias_opt)?;
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            7 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Ortamı seçin")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                let bin_path: String = Input::with_theme(&theme)
                    .with_prompt("Kopyalanacak dosya/binary yolu")
                    .interact_text()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                if bin_path.trim().is_empty() {
                    continue;
                }
                register_binary(env_name, bin_path.trim())?;
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            8 => {
                let envs = get_envs_list()?;
                if envs.is_empty() {
                    println!("\x1b[1;31mMevcut izole ortam bulunamadı.\x1b[0m");
                    println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                    let mut stdout = io::stdout();
                    stdout.flush()?;
                    let mut buffer = String::new();
                    io::stdin().read_line(&mut buffer)?;
                    continue;
                }
                let sel = Select::with_theme(&theme)
                    .with_prompt("Silmek istediğiniz ortamı seçin")
                    .default(0)
                    .items(&envs)
                    .interact_opt()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                
                let env_name = match sel {
                    Some(s) => &envs[s],
                    None => continue,
                };

                let confirm = Confirm::with_theme(&theme)
                    .with_prompt(format!("'{}' ortamını silmek istediğinizden emin misiniz?", env_name))
                    .default(false)
                    .interact()
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                if confirm {
                    delete_env(env_name)?;
                } else {
                    println!("Silme işlemi iptal edildi.");
                }
                println!("\n\x1b[33mDevam etmek için Enter'a basın...\x1b[0m");
                let mut stdout = io::stdout();
                stdout.flush()?;
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
            }
            9 => {
                println!("\x1b[1;32mGörüşmek üzere!\x1b[0m");
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

fn get_envs_list() -> io::Result<Vec<String>> {
    let envs_dir = get_envs_dir();
    let mut list = Vec::new();
    if envs_dir.exists() && envs_dir.is_dir() {
        for entry in fs::read_dir(envs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    list.push(name.to_string());
                }
            }
        }
    }
    list.sort();
    Ok(list)
}

fn get_env_binaries(env_name: &str) -> io::Result<Vec<String>> {
    let env_path = get_env_path(env_name);
    let bin_dir = env_path.join("usr/bin");
    let mut list = Vec::new();
    if bin_dir.exists() && bin_dir.is_dir() {
        for entry in fs::read_dir(bin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    list.push(name.to_string());
                }
            }
        }
    }
    list.sort();
    Ok(list)
}

fn parse_missing_library(stderr: &str) -> Option<String> {
    if let Some(idx) = stderr.find("error while loading shared libraries: ") {
        let rest = &stderr[idx + "error while loading shared libraries: ".len()..];
        if let Some(colon_idx) = rest.find(':') {
            return Some(rest[..colon_idx].trim().to_string());
        }
    }
    None
}

fn find_package_for_library(lib_name: &str, is_steam: bool) -> Option<String> {
    let pm = detect_package_manager();
    match pm {
        PackageManager::Dnf => {
            let output = Command::new("dnf")
                .args(["provides", &format!("*/{}", lib_name)])
                .output()
                .ok()?;

            if !output.status.success() {
                return None;
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut packages = Vec::new();

            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.contains(" : ") && !trimmed.starts_with("Repo") && !trimmed.starts_with("Matched") && !trimmed.starts_with("Provide") && !trimmed.starts_with("Filename") {
                    if let Some(pkg) = trimmed.split_whitespace().next() {
                        packages.push(pkg.to_string());
                    }
                }
            }

            if packages.is_empty() {
                return None;
            }

            if is_steam || lib_name.contains("i686") || lib_name.contains("32") {
                if let Some(pkg) = packages.iter().find(|pkg| pkg.contains(".i686")) {
                    return Some(pkg.clone());
                }
            } else {
                if let Some(pkg) = packages.iter().find(|pkg| pkg.contains(".x86_64")) {
                    return Some(pkg.clone());
                }
            }

            Some(packages[0].clone())
        }
        PackageManager::Apt => {
            let output = Command::new("apt-file")
                .args(["search", lib_name])
                .output();
            if let Ok(out) = output {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    for line in stdout.lines() {
                        if let Some(colon_idx) = line.find(':') {
                            let pkg = line[..colon_idx].trim().to_string();
                            if is_steam || lib_name.contains("i386") || lib_name.contains("32") {
                                if pkg.contains(":i386") {
                                    return Some(pkg);
                                }
                            } else {
                                if !pkg.contains(":i386") {
                                    return Some(pkg);
                                }
                            }
                            return Some(pkg);
                        }
                    }
                }
            }
            None
        }
        PackageManager::Pacman => {
            let output = Command::new("pacman")
                .args(["-F", lib_name])
                .output();
            if let Ok(out) = output {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    for line in stdout.lines() {
                        if line.contains(" is owned by ") {
                            let parts: Vec<&str> = line.split(" is owned by ").collect();
                            if parts.len() >= 2 {
                                let pkg_info = parts[1].trim();
                                if let Some(pkg_name) = pkg_info.split_whitespace().next() {
                                    if let Some(slash_idx) = pkg_name.find('/') {
                                        return Some(pkg_name[slash_idx+1..].to_string());
                                    } else {
                                        return Some(pkg_name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None
        }
    }
}

fn align_nvidia_drivers_if_needed(env_name: &str) -> io::Result<()> {
    let kernel_version = get_nvidia_kernel_version();
    if kernel_version.is_none() {
        return Ok(());
    }
    let kv = kernel_version.unwrap();

    let env_path = get_env_path(env_name);
    let lib_dir = env_path.join("usr/lib");
    if !lib_dir.exists() {
        return Ok(());
    }

    let mut has_nvidia_lib = false;
    let mut matches_kernel = false;
    let mut found_version = String::new();

    if let Ok(entries) = fs::read_dir(lib_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("libGLX_nvidia.so.") {
                    has_nvidia_lib = true;
                    let ver = &name["libGLX_nvidia.so.".len()..];
                    if ver == kv {
                        matches_kernel = true;
                    } else {
                        found_version = ver.to_string();
                    }
                }
            }
        }
    }

    if has_nvidia_lib && !matches_kernel {
        println!("\x1b[1;33m[İzole Grafik] Nvidia sürücü uyuşmazlığı tespit edildi (Host: {}, Ortam: {}).\x1b[0m", kv, found_version);
        println!("\x1b[1;34m[İzole Grafik] Kütüphaneler güncelleniyor, lütfen bekleyin...\x1b[0m");
        install_packages(env_name, &[
            "xorg-x11-drv-nvidia-libs.i686".to_string(),
            "xorg-x11-drv-nvidia-cuda-libs.i686".to_string()
        ])?;
        println!("\x1b[1;32m[İzole Grafik] Sürücüler başarıyla güncellendi!\x1b[0m");
    }

    Ok(())
}

fn get_nvidia_kernel_version() -> Option<String> {
    let content = fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    for token in content.split_whitespace() {
        if token.contains('.') && token.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Some(token.to_string());
        }
    }
    None
}

#[derive(Deserialize, Debug)]
struct AiDiagnosis {
    explanation: String,
    fix_action: String,
    package_name: Option<String>,
    command_to_run: Option<String>,
}

fn run_ollama_diagnosis(command: &str, stderr: &str) -> Option<AiDiagnosis> {
    let status = Command::new("curl")
        .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "http://localhost:11434"])
        .output()
        .ok()?;
        
    let code = String::from_utf8_lossy(&status.stdout).trim().to_string();
    if code != "200" {
        return None;
    }

    let model = get_first_ollama_model().unwrap_or_else(|| "llama3".to_string());
    let pm = detect_package_manager();
    let pm_desc = match pm {
        PackageManager::Dnf => "DNF paket adı",
        PackageManager::Apt => "APT (deb) paket adı",
        PackageManager::Pacman => "pacman paket adı",
    };
    let pm_cmd_example = match pm {
        PackageManager::Dnf => "sudo dnf install ...",
        PackageManager::Apt => "sudo apt-get install ...",
        PackageManager::Pacman => "sudo pacman -S ...",
    };

    let prompt = format!(
        "Sen bir Linux ve Bubblewrap sandbox uzmanı yapay zeka asistanısın. \
        İzole ortamda çalıştırılan '{}' komutu şu hata çıktısıyla sonlandı:\n\n{}\n\n\
        Lütfen bu hatayı analiz et ve çözmek için ne yapılması gerektiğini bul. \
        Cevabını MUTLAKA aşağıdaki JSON formatında ver. JSON dışında hiçbir metin, markdown veya açıklama yazma.\n\n\
        Format:\n\
        {{\n\
          \"explanation\": \"Hatanın kısa Türkçe açıklaması (maksimum 2 cümle).\",\n\
          \"fix_action\": \"install_container_package\" veya \"run_command\" veya \"none\",\n\
          \"package_name\": \"Eğer kurulması gereken bir {} varsa yaz, yoksa null\",\n\
          \"command_to_run\": \"Eğer çalıştırılması gereken bir komut varsa (örn: {} veya mkdir ...), yoksa null\"\n\
        }}",
        command, stderr, pm_desc, pm_cmd_example
    );
    
    let payload = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "format": "json"
    });

    let payload_str = serde_json::to_string(&payload).ok()?;

    let response = Command::new("curl")
        .args([
            "-s",
            "-X", "POST",
            "-H", "Content-Type: application/json",
            "-d", &payload_str,
            "http://localhost:11434/api/generate"
        ])
        .output()
        .ok()?;

    if !response.status.success() {
        return None;
    }

    #[derive(Deserialize)]
    struct OllamaResponse {
        response: String,
    }

    let res: OllamaResponse = serde_json::from_slice(&response.stdout).ok()?;
    let diagnosis: AiDiagnosis = serde_json::from_str(&res.response).ok()?;
    Some(diagnosis)
}

fn get_first_ollama_model() -> Option<String> {
    let output = Command::new("curl")
        .args(["-s", "http://localhost:11434/api/tags"])
        .output()
        .ok()?;
        
    if !output.status.success() {
        return None;
    }

    #[derive(Deserialize)]
    struct TagModel {
        name: String,
    }
    #[derive(Deserialize)]
    struct TagsResponse {
        models: Vec<TagModel>,
    }

    let tags: TagsResponse = serde_json::from_slice(&output.stdout).ok()?;
    tags.models.first().map(|m| m.name.clone())
}
