use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foto_core::{Config, Filter, Foto, NoPlatform, Policy};

struct Node {
    foto: Arc<Foto>,
    dir: PathBuf,
}

impl Node {
    fn open(base: &Path, name: &str, originals_all: bool) -> Node {
        let dir = base.join(name);
        let foto = Foto::open(
            Config {
                data_dir: dir.join("data").to_string_lossy().into_owned(),
                library_dir: dir.join("Pictures").to_string_lossy().into_owned(),
                device_name: name.into(),
                platform: "test".into(),
                originals_all,
            },
            Arc::new(NoPlatform),
        )
        .expect("open");
        Node { foto, dir }
    }

    fn lib(&self) -> String {
        self.foto.libraries()[0].id.clone()
    }

    fn count(&self, filter: Filter) -> usize {
        self.foto.timeline(self.lib(), filter).len()
    }

    fn library_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect(&self.dir.join("Pictures"), &mut out);
        out
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn wait(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn photo(path: &Path, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([seed, x as u8 * 3, y as u8 * 5]));
    img.save(path).unwrap();
}

fn base(name: &str) -> PathBuf {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let b = std::env::temp_dir().join(format!("foto-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&b);
    std::fs::create_dir_all(&b).unwrap();
    b.canonicalize().unwrap()
}

fn pair(base: &Path) -> (Node, Node, PathBuf) {
    let a = Node::open(base, "a", true);
    let lib = a.foto.create_library("Family".into()).unwrap().id;
    let src = base.join("src");
    for i in 0..3 {
        photo(&src.join(format!("img{i}.png")), i * 40);
    }
    a.foto
        .add_root(lib.clone(), src.to_string_lossy().into_owned())
        .unwrap();
    wait("a imports", || a.count(Filter::All) == 3);
    let b = Node::open(base, "b", true);
    let ticket = a.foto.share(lib.clone(), true).unwrap();
    b.foto.join(ticket).unwrap();
    (a, b, src)
}

#[test]
fn sync_delete_restore_edit_rename() {
    let base = base("main");
    let (a, b, src) = pair(&base);
    let lib = a.lib();
    assert_eq!(b.lib(), lib);
    wait("b gets name", || b.foto.libraries()[0].name == "Family");
    wait("b gets assets", || b.count(Filter::All) == 3);
    wait("b gets thumbnails", || b.foto.status().thumbs_pending == 0);
    let first = b.foto.timeline(lib.clone(), Filter::All)[0].hash.clone();
    assert!(
        b.foto
            .thumbnail(lib.clone(), first.clone(), false)
            .is_some()
    );
    wait("b gets originals", || {
        let s = b.foto.status();
        assert!(s.paused.is_none(), "originals paused: {:?}", s.paused);
        b.library_files().len() == 3
    });
    let devices = a.foto.devices(lib.clone());
    wait("devices known", || a.foto.devices(lib.clone()).len() == 2);
    assert!(devices.iter().any(|d| d.this_device));

    std::fs::rename(src.join("img0.png"), src.join("renamed.png")).unwrap();
    std::thread::sleep(Duration::from_secs(4));
    a.foto.scan_now().unwrap();
    assert_eq!(a.count(Filter::All), 3, "rename must not delete");

    let victim = a
        .foto
        .timeline(lib.clone(), Filter::All)
        .into_iter()
        .find(|s| a.foto.asset(lib.clone(), s.hash.clone()).unwrap().name == "img1.png")
        .unwrap()
        .hash;
    a.foto.delete(lib.clone(), vec![victim.clone()]).unwrap();
    assert!(!src.join("img1.png").exists());
    assert!(src.join(".foto-trash/img1.png").exists());
    wait("b sees delete", || {
        b.count(Filter::All) == 2 && b.count(Filter::Deleted) == 1
    });
    wait("b trashes file", || {
        b.library_files()
            .iter()
            .filter(|p| !p.to_string_lossy().contains(".foto-trash"))
            .count()
            == 2
    });

    a.foto.restore(lib.clone(), vec![victim.clone()]).unwrap();
    assert!(src.join("img1.png").exists());
    wait("b sees restore", || b.count(Filter::All) == 3);
    wait("b restores file", || {
        b.library_files()
            .iter()
            .filter(|p| !p.to_string_lossy().contains(".foto-trash"))
            .count()
            == 3
    });

    photo(&src.join("img2.png"), 250);
    a.foto.scan_now().unwrap();
    wait("edit replaces on a", || {
        a.count(Filter::All) == 3 && a.count(Filter::Deleted) == 1
    });
    wait("edit replaces on b", || {
        b.count(Filter::All) == 3 && b.count(Filter::Deleted) == 1
    });

    let fav = a.foto.timeline(lib.clone(), Filter::All)[0].hash.clone();
    a.foto.set_favorite(lib.clone(), fav, true).unwrap();
    wait("favorite syncs", || b.count(Filter::Favorites) == 1);

    let moved = base.join("src-away");
    std::fs::rename(&src, &moved).unwrap();
    a.foto.scan_now().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(a.count(Filter::All), 3, "offline root must not delete");
    std::fs::rename(&moved, &src).unwrap();

    let received = b
        .library_files()
        .into_iter()
        .find(|p| !p.to_string_lossy().contains(".foto-trash"))
        .unwrap();
    std::fs::remove_file(&received).unwrap();
    wait("delete in received folder propagates", || {
        a.count(Filter::All) == 2
    });

    b.foto.shutdown();
    drop(b);
    photo(&src.join("new.png"), 7);
    a.foto.scan_now().unwrap();
    wait("a imports new", || a.count(Filter::All) == 3);
    let b = Node::open(&base, "b", true);
    wait("b catches up after restart", || b.count(Filter::All) == 3);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn archive_on_demand_and_read_only() {
    let base = base("roles");
    let (a, b, src) = pair(&base);
    let lib = a.lib();
    b.foto
        .set_policy(Policy {
            originals_all: false,
            archive: false,
            metered_ok: false,
        })
        .unwrap();
    wait("b gets assets", || b.count(Filter::All) == 3);
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        b.library_files().is_empty(),
        "on-demand must not download everything"
    );
    let h = b.foto.timeline(lib.clone(), Filter::All)[0].hash.clone();
    wait("on-demand original", || {
        b.foto.original(lib.clone(), h.clone()).is_some()
    });

    let c = Node::open(&base, "c", true);
    c.foto
        .join(a.foto.share(lib.clone(), true).unwrap())
        .unwrap();
    c.foto
        .set_policy(Policy {
            originals_all: true,
            archive: true,
            metered_ok: false,
        })
        .unwrap();
    wait("c gets originals", || c.library_files().len() == 3);
    let victim = a.foto.timeline(lib.clone(), Filter::All)[0].hash.clone();
    a.foto.delete(lib.clone(), vec![victim]).unwrap();
    wait("c sees delete", || c.count(Filter::All) == 2);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(c.library_files().len(), 3, "archive keeps files");

    let r = Node::open(&base, "r", false);
    r.foto
        .join(a.foto.share(lib.clone(), false).unwrap())
        .unwrap();
    assert!(!r.foto.libraries()[0].writable);
    wait("reader gets assets", || r.count(Filter::All) == 2);
    let h = r.foto.timeline(lib.clone(), Filter::All)[0].hash.clone();
    assert!(r.foto.delete(lib.clone(), vec![h]).is_err());
    assert!(
        r.foto
            .add_root(lib, src.to_string_lossy().into_owned())
            .is_err()
    );
    let _ = std::fs::remove_dir_all(base);
}
