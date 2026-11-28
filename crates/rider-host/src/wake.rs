use std::net::Ipv4Addr;

use rider_core::wake::{self, Mac};

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub wired: Vec<Mac>,
    pub wired_names: Vec<String>,
    pub broadcasts: Vec<Ipv4Addr>,
}

pub fn wake(macs: &[Mac]) -> anyhow::Result<usize> {
    let broadcasts = network().broadcasts;
    wake::send(macs, &broadcasts)
}

#[cfg(windows)]
pub fn network() -> Network {
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    const ETHERNET: u32 = 6;
    const BUFFER_OVERFLOW: u32 = 111;

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        buf.resize((size as usize).div_ceil(8), 0);
        let err = unsafe {
            GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(buf.as_mut_ptr().cast()), &raw mut size)
        };
        match err {
            0 => break,
            BUFFER_OVERFLOW => {}
            _ => return Network::default(),
        }
    }
    let mut out = Network::default();
    unsafe {
        let mut adapter = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            let a = &*adapter;
            if a.IfType == ETHERNET && a.PhysicalAddressLength == 6 {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&a.PhysicalAddress[..6]);
                out.wired.push(Mac(mac));
                out.wired_names.extend(a.Description.to_string().ok());
            }
            if a.OperStatus == IfOperStatusUp {
                let mut unicast = a.FirstUnicastAddress;
                while !unicast.is_null() {
                    let u = &*unicast;
                    let sa = u.Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = std::ptr::read_unaligned(sa.cast::<SOCKADDR_IN>());
                        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                        out.broadcasts.extend(wake::directed_broadcast(ip, u.OnLinkPrefixLength));
                    }
                    unicast = u.Next;
                }
            }
            adapter = a.Next;
        }
    }
    out.wired = wake::usable(&out.wired);
    out.broadcasts.dedup();
    out
}

#[cfg(not(windows))]
pub fn network() -> Network {
    Network::default()
}

#[cfg(windows)]
pub fn allow_wake() {
    let wired = network().wired_names;
    if wired.is_empty() {
        return;
    }
    let programmable = powercfg(&["/devicequery", "wake_programmable"]).unwrap_or_default();
    let armed = powercfg(&["/devicequery", "wake_armed"]).unwrap_or_default();
    for name in to_arm(&wired, &programmable, &armed) {
        if powercfg(&["/deviceenablewake", name]).is_some() {
            tracing::info!("allowed \"{name}\" to wake this computer");
        } else {
            tracing::warn!("cannot allow \"{name}\" to wake this computer");
        }
    }
}

#[cfg(windows)]
fn powercfg(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;
    let out = std::process::Command::new("powercfg").args(args).creation_flags(CREATE_NO_WINDOW.0).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(any(windows, test))]
fn to_arm<'a>(wired: &[String], programmable: &'a str, armed: &str) -> Vec<&'a str> {
    let armed: Vec<&str> = armed.lines().map(str::trim).collect();
    programmable
        .lines()
        .map(str::trim)
        .filter(|name| wired.iter().any(|w| w == name) && !armed.contains(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wired_adapters_not_yet_armed_are_armed() {
        let wired =
            vec!["Intel(R) Ethernet Connection I219-V".to_owned(), "Realtek PCIe GbE Family Controller".to_owned()];
        let programmable = [
            "HID Keyboard Device",
            "Intel(R) Ethernet Connection I219-V",
            "Realtek PCIe GbE Family Controller",
            "Intel(R) Wi-Fi 6 AX201 160MHz",
        ]
        .join("\r\n");
        let armed = "HID Keyboard Device\r\nRealtek PCIe GbE Family Controller\r\n";
        assert_eq!(to_arm(&wired, &programmable, armed), vec!["Intel(R) Ethernet Connection I219-V"]);
        assert_eq!(to_arm(&[], &programmable, ""), Vec::<&str>::new());
    }
}
