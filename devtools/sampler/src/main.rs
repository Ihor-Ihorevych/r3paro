use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Diagnostics::Debug::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Threading::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (exe, pdb_path) = (&args[1], &args[2]);
    let mut child = std::process::Command::new(exe).args(&args[3..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(300));
    unsafe {
        let proc = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
        // module base
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        let mut me: MODULEENTRY32W = zeroed();
        me.dwSize = size_of::<MODULEENTRY32W>() as u32;
        Module32FirstW(snap, &mut me);
        let base = me.modBaseAddr as u64;
        CloseHandle(snap);
        // threads: lowest tid
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        let mut te: THREADENTRY32 = zeroed();
        te.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut tids = vec![];
        if Thread32First(snap, &mut te) != 0 {
            loop {
                if te.th32OwnerProcessID == pid {
                    tids.push(te.th32ThreadID);
                }
                if Thread32Next(snap, &mut te) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        tids.sort();
        let ths: Vec<_> = tids.iter().map(|&t| OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, t)).collect();
        eprintln!("pid {pid} tids {tids:?} base {base:#x}");
        let mut samples: Vec<Vec<u64>> = vec![];
        let delay: u64 = std::env::var("SAMPLER_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        std::thread::sleep(std::time::Duration::from_millis(delay));
        let maxms: u64 = std::env::var("SAMPLER_MAXMS").ok().and_then(|v| v.parse().ok()).unwrap_or(u64::MAX);
        let began = std::time::Instant::now();
        loop {
            if began.elapsed().as_millis() as u64 > maxms {
                let _ = child.kill();
                break;
            }
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            for &th in &ths {
            if SuspendThread(th) == u32::MAX {
                continue;
            }
            #[repr(align(16))]
            struct Ctx(CONTEXT);
            let mut c: Ctx = zeroed();
            c.0.ContextFlags = 0x10_0001; // CONTEXT_CONTROL
            let mut st = vec![];
            if GetThreadContext(th, &mut c.0) != 0 {
                st.push(c.0.Rip);
                let mut rbp = c.0.Rbp;
                for _ in 0..64 {
                    let mut buf = [0u64; 2];
                    let mut n = 0usize;
                    if rbp < 0x10000 || rbp & 7 != 0 || ReadProcessMemory(proc, rbp as *const _, buf.as_mut_ptr() as *mut _, 16, &mut n) == 0 || n != 16 {
                        break;
                    }
                    if buf[1] < base || buf[1] > base + 0x4000_0000 {
                        break;
                    }
                    st.push(buf[1]);
                    if buf[0] <= rbp {
                        break;
                    }
                    rbp = buf[0];
                }
            }
            ResumeThread(th);
            if !st.is_empty() && st[0] >= base && st[0] < base + 0x4000_0000 {
                samples.push(st);
            }
            }
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
        eprintln!("{} samples", samples.len());
        let file = std::fs::File::open(pdb_path).unwrap();
        let pdb = pdb_addr2line::pdb::PDB::open(file).unwrap();
        let data = pdb_addr2line::ContextPdbData::try_from_pdb(pdb).unwrap();
        let ctx = data.make_context().unwrap();
        let mut cache: HashMap<u64, Vec<String>> = HashMap::new();
        let mut resolve = |a: u64| -> Vec<String> {
            cache
                .entry(a)
                .or_insert_with(|| {
                    let rva = a.wrapping_sub(base) as u32;
                    match ctx.find_frames(rva.saturating_sub(if a == 0 { 0 } else { 0 })) {
                        Ok(Some(f)) => f.frames.iter().map(|fr| fr.function.clone().unwrap_or("?".into())).collect(),
                        _ => vec![format!("?{a:#x}")],
                    }
                })
                .clone()
        };
        if let Ok(root) = std::env::var("SAMPLER_ROOT") {
            let before = samples.len();
            samples.retain(|s| s.iter().enumerate().any(|(i, a)| resolve(if i == 0 { *a } else { a - 1 }).iter().any(|n| n.contains(&root))));
            eprintln!("{} of {before} samples under {root}", samples.len());
        }
        let mut self_t: HashMap<String, u32> = HashMap::new();
        let mut incl: HashMap<String, u32> = HashMap::new();
        for s in &samples {
            let mut seen = std::collections::HashSet::new();
            for (i, a) in s.iter().enumerate() {
                // return addresses point after the call: look up the call instruction
                let q = if i == 0 { *a } else { a - 1 };
                let names = resolve(q);
                if i == 0 {
                    *self_t.entry(names[0].clone()).or_default() += 1;
                }
                for n in names {
                    if seen.insert(n.clone()) {
                        *incl.entry(n).or_default() += 1;
                    }
                }
            }
        }
        if let Ok(leaves) = std::env::var("SAMPLER_LEAF") {
            for leaf in leaves.split(',') {
                let mut chains: HashMap<String, u32> = HashMap::new();
                for s in &samples {
                    let mut names: Vec<String> = vec![];
                    for (i, a) in s.iter().enumerate() {
                        let q = if i == 0 { *a } else { a - 1 };
                        names.extend(resolve(q));
                    }
                    if names.first().is_some_and(|n| n.contains(leaf)) {
                        let mut ch: Vec<String> = vec![];
                        for n in names.iter().skip(1) {
                            if ch.last() != Some(n) && !n.contains(leaf) && !n.starts_with("core::") && !n.starts_with("alloc::") {
                                ch.push(n.chars().take(60).collect());
                            }
                            if ch.len() == 8 {
                                break;
                            }
                        }
                        *chains.entry(ch.join(" < ")).or_default() += 1;
                    }
                }
                let mut v: Vec<_> = chains.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                println!("== callers of {leaf}");
                for (k, c) in v.iter().take(12) {
                    println!("{:6.2}% {k}", *c as f64 * 100.0 / samples.len() as f64);
                }
            }
        }
        let n = samples.len() as f64;
        let top = |m: &HashMap<String, u32>, title: &str| {
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            println!("== {title}");
            for (k, c) in v.iter().take(400) {
                let k: String = k.chars().take(150).collect();
                println!("{:6.2}% {k}", **c as f64 * 100.0 / n);
            }
        };
        top(&self_t, "self");
        top(&incl, "inclusive");
    }
}
