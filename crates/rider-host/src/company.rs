use crate::config::{CompanyLink, HostConfig};
use anyhow::{Context, Result};
use rider_core::company::{HostInfo, JoinToken, NetLink, Whereabouts};
use rider_core::identity::{Identity, fp_hex};
use rider_core::rendezvous::ServerRef;
use windows::Win32::System::RemoteDesktop::{
    WTS_SESSIONSTATE_LOCK, WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSINFOEXW, WTSQuerySessionInformationW,
    WTSSessionInfoEx, WTSUserName,
};
use windows::core::PWSTR;

pub fn enroll(cfg: &mut HostConfig, identity: &Identity, token: &str, unattended: bool) -> Result<String> {
    let token = JoinToken::decode(token)?;
    let server = ServerRef::new(&token.server, &token.server_fingerprint_hex())?;
    let link = CompanyLink { name: String::new(), admin: fp_hex(&token.admin), unattended };
    let info = info(cfg, Some(&link));
    let company = tokio::runtime::Runtime::new()?
        .block_on(rider_core::rendezvous::enroll(identity, &server, &token.secret, info))
        .context("joining the company")?;
    cfg.server = Some(server);
    cfg.company = Some(CompanyLink { name: company.clone(), ..link });
    cfg.save()?;
    Ok(company)
}

pub fn info(cfg: &HostConfig, company: Option<&CompanyLink>) -> HostInfo {
    let (user, locked) = console_user();
    HostInfo {
        name: cfg.name.clone(),
        user,
        locked,
        version: env!("CARGO_PKG_VERSION").into(),
        unattended: company.is_some_and(|c| c.unattended),
    }
}

pub fn console_user() -> (Option<String>, bool) {
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        let mut buf = PWSTR::null();
        let mut len = 0u32;
        let user = if WTSQuerySessionInformationW(None, session, WTSUserName, &raw mut buf, &raw mut len).is_ok() {
            let name = buf.to_string().unwrap_or_default();
            WTSFreeMemory(buf.0.cast());
            Some(name).filter(|n| !n.is_empty())
        } else {
            None
        };
        let mut locked = false;
        let mut buf = PWSTR::null();
        if user.is_some()
            && WTSQuerySessionInformationW(None, session, WTSSessionInfoEx, &raw mut buf, &raw mut len).is_ok()
        {
            if len as usize >= size_of::<WTSINFOEXW>() {
                let ex = std::ptr::read_unaligned(buf.0.cast::<WTSINFOEXW>());
                if ex.Level == 1 {
                    locked = ex.Data.WTSInfoExLevel1.SessionFlags as u32 == WTS_SESSIONSTATE_LOCK;
                }
            }
            WTSFreeMemory(buf.0.cast());
        }
        (user, locked)
    }
}

pub fn whereabouts() -> Whereabouts {
    let links = net_links().unwrap_or_else(|e| {
        tracing::debug!("network list: {e:#}");
        Vec::new()
    });
    Whereabouts::decide(&links, domain_joined())
}

fn net_links() -> Result<Vec<NetLink>> {
    use windows::Win32::Networking::NetworkListManager::{
        INetworkConnection, INetworkListManager, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED, NetworkListManager,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };

    let adapters = adapters();
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> Result<Vec<NetLink>> {
            let nlm: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
            let connections = nlm.GetNetworkConnections()?;
            let mut links = Vec::new();
            loop {
                let mut item: [Option<INetworkConnection>; 1] = [None];
                let mut fetched = 0u32;
                if connections.Next(&mut item, Some(&raw mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(c) = item[0].take() else { break };
                if !c.IsConnected().is_ok_and(windows::Win32::Foundation::VARIANT_BOOL::as_bool) {
                    continue;
                }
                let network = c.GetNetwork()?;
                let (description, if_type) = adapters.get(&c.GetAdapterId()?.to_u128()).cloned().unwrap_or_default();
                links.push(NetLink {
                    network: network.GetName().map_or_default(|n| n.to_string()),
                    vpn: rider_core::company::is_vpn_adapter(&description, if_type),
                    adapter: description,
                    domain: network.GetCategory()? == NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
                });
            }
            Ok(links)
        })();
        if initialised {
            CoUninitialize();
        }
        result
    }
}

fn adapters() -> std::collections::HashMap<u128, (String, u32)> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GAA_FLAG_SKIP_UNICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;

    let mut out = std::collections::HashMap::new();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_SKIP_UNICAST;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    for _ in 0..3 {
        buf.resize((size as usize).div_ceil(8), 0);
        let err = unsafe {
            GetAdaptersAddresses(u32::from(AF_UNSPEC.0), flags, None, Some(buf.as_mut_ptr().cast()), &raw mut size)
        };
        match err {
            0 => break,
            111 => {}
            _ => return out,
        }
    }
    unsafe {
        let mut adapter = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            let a = &*adapter;
            let id = a.AdapterName.to_string().ok().map(|s| s.trim_matches(['{', '}']).to_owned());
            if let Some(guid) = id.and_then(|s| windows::core::GUID::try_from(s.as_str()).ok()) {
                let description = a.Description.to_string().unwrap_or_default();
                out.insert(guid.to_u128(), (description, a.IfType));
            }
            adapter = a.Next;
        }
    }
    out
}

fn domain_joined() -> bool {
    use windows::Win32::NetworkManagement::NetManagement::{
        NETSETUP_JOIN_STATUS, NetApiBufferFree, NetGetJoinInformation, NetSetupDomainName,
    };
    let mut name = PWSTR::null();
    let mut status = NETSETUP_JOIN_STATUS(0);
    unsafe {
        let err = NetGetJoinInformation(windows::core::PCWSTR::null(), &raw mut name, &raw mut status);
        if !name.is_null() {
            NetApiBufferFree(Some(name.0.cast()));
        }
        err == 0 && status == NetSetupDomainName
    }
}
