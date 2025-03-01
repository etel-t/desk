use std::{
    collections::HashMap,
    iter::FromIterator,
    process::Child,
    sync::{Arc, Mutex},
};
//Modules used for posting the json data to server & validating the expiry date
use reqwest;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value as JsonValue;
use chrono::NaiveDate;
use chrono::{ Local};
use chrono::Utc;
use hbb_common::{
    config::Config,
};
use sciter::Value;
extern crate machine_uid;
use whoami;
extern crate winapi;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
//Modules used for posting the json data to server & validating the expiry date
use hbb_common::{
    allow_err,
    config::{self, PeerConfig},
    log,
};

#[cfg(not(any(feature = "flutter", feature = "cli")))]
use crate::ui_session_interface::Session;
use crate::{common::get_app_name, ipc, ui_interface::*};

mod cm;
#[cfg(feature = "inline")]
pub mod inline;
#[cfg(target_os = "macos")]
mod macos;
pub mod remote;
#[cfg(target_os = "windows")]
pub mod win_privacy;

pub type Children = Arc<Mutex<(bool, HashMap<(String, String), Child>)>>;
#[allow(dead_code)]
type Status = (i32, bool, i64, String);

lazy_static::lazy_static! {
    // stupid workaround for https://sciter.com/forums/topic/crash-on-latest-tis-mac-sdk-sometimes/
    static ref STUPID_VALUES: Mutex<Vec<Arc<Vec<Value>>>> = Default::default();
}

#[cfg(not(any(feature = "flutter", feature = "cli")))]
lazy_static::lazy_static! {
    pub static ref CUR_SESSION: Arc<Mutex<Option<Session<remote::SciterHandler>>>> = Default::default();
}

struct UIHostHandler;




pub fn start(args: &mut [String]) {
    #[cfg(target_os = "macos")]
    macos::show_dock();
    #[cfg(all(target_os = "linux", feature = "inline"))]
    {
        #[cfg(feature = "appimage")]
        let prefix = std::env::var("APPDIR").unwrap_or("".to_string());
        #[cfg(not(feature = "appimage"))]
        let prefix = "".to_string();
        #[cfg(feature = "flatpak")]
        let dir = "/app";
        #[cfg(not(feature = "flatpak"))]
        let dir = "/usr";
        sciter::set_library(&(prefix + dir + "/lib/rustdesk/libsciter-gtk.so")).ok();
    }
    // https://github.com/c-smile/sciter-sdk/blob/master/include/sciter-x-types.h
    // https://github.com/rustdesk/rustdesk/issues/132#issuecomment-886069737
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::GfxLayer(
        sciter::GFX_LAYER::WARP
    )));
    #[cfg(all(windows, not(feature = "inline")))]
    unsafe {
        winapi::um::shellscalingapi::SetProcessDpiAwareness(2);
    }
    use sciter::SCRIPT_RUNTIME_FEATURES::*;
    allow_err!(sciter::set_options(sciter::RuntimeOptions::ScriptFeatures(
        ALLOW_FILE_IO as u8 | ALLOW_SOCKET_IO as u8 | ALLOW_EVAL as u8 | ALLOW_SYSINFO as u8
    )));
    let mut frame = sciter::WindowBuilder::main_window().create();
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::UxTheming(true)));
    frame.set_title(&crate::get_app_name());
    #[cfg(target_os = "macos")]
    macos::make_menubar(frame.get_host(), args.is_empty());
    let page;
    if args.len() > 1 && args[0] == "--play" {
        args[0] = "--connect".to_owned();
        let path: std::path::PathBuf = (&args[1]).into();
        let id = path
            .file_stem()
            .map(|p| p.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned();
        args[1] = id;
    }
    if args.is_empty() {
        let children: Children = Default::default();
        std::thread::spawn(move || check_zombie(children));
        crate::common::check_software_update();
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "index.html";
    } else if args[0] == "--install" {
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "install.html";
    } else if args[0] == "--cm" {
        frame.register_behavior("connection-manager", move || {
            Box::new(cm::SciterConnectionManager::new())
        });
        page = "cm.html";
    } else if (args[0] == "--connect"
        || args[0] == "--file-transfer"
        || args[0] == "--port-forward"
        || args[0] == "--rdp")
        && args.len() > 1
    {
        #[cfg(windows)]
        {
            let hw = frame.get_host().get_hwnd();
            crate::platform::windows::enable_lowlevel_keyboard(hw as _);
        }
        let mut iter = args.iter();
        let cmd = iter.next().unwrap().clone();
        let id = iter.next().unwrap().clone();
        let pass = iter.next().unwrap_or(&"".to_owned()).clone();
        let args: Vec<String> = iter.map(|x| x.clone()).collect();
        frame.set_title(&id);
        frame.register_behavior("native-remote", move || {
            let handler =
                remote::SciterSession::new(cmd.clone(), id.clone(), pass.clone(), args.clone());
            #[cfg(not(any(feature = "flutter", feature = "cli")))]
            {
                *CUR_SESSION.lock().unwrap() = Some(handler.inner());
            }
            Box::new(handler)
        });
        page = "remote.html";
    } else {
        log::error!("Wrong command: {:?}", args);
        return;
    }
    #[cfg(feature = "inline")]
    {
        let html = if page == "index.html" {
            inline::get_index()
        } else if page == "cm.html" {
            inline::get_cm()
        } else if page == "install.html" {
            inline::get_install()
        } else {
            inline::get_remote()
        };
        frame.load_html(html.as_bytes(), Some(page));
    }
    #[cfg(not(feature = "inline"))]
    frame.load_file(&format!(
        "file://{}/src/ui/{}",
        std::env::current_dir()
            .map(|c| c.display().to_string())
            .unwrap_or("".to_owned()),
        page
    ));
    frame.run_app();
}

struct UI {}

impl UI {
    fn recent_sessions_updated(&self) -> bool {
        recent_sessions_updated()
    }

    fn get_id(&self) -> String {
        ipc::get_id()
    }

    fn temporary_password(&mut self) -> String {
        temporary_password()
    }

    fn update_temporary_password(&self) {
        update_temporary_password()
    }

    fn permanent_password(&self) -> String {
        permanent_password()
    }

    fn set_permanent_password(&self, password: String) {
        set_permanent_password(password);
    }

    //below functions for set & get of login_id(registered_mail_id), login_passowrd(registered_password), email_id, license_key & expiry_date.
    fn set_login_id(&self, login_id: String) {
        set_login_id(login_id);
    }

    fn get_login_id(&self) -> String  {
        get_login_id()
    }

    fn set_email_id(&self, email_id: String) {
        set_email_id(email_id);
    }

    fn get_email_id(&self) -> String  {
        get_email_id()
    }

    fn set_expiry_date(&self, expiry_date: String) {
        set_expiry_date(expiry_date);
    }

    fn get_expiry_date(&self) -> String  {
        get_expiry_date()
    }


    fn set_license_key(&self, key_license: String) {
        set_license_key(key_license);
    }

    fn get_license_key(&self) -> String  {
        get_license_key()
    }
    
    fn set_login_password(&self, login_password: String) {
        set_login_password(login_password);
    }

    fn get_login_password(&self) -> String {
        get_login_password()
    }

    //this function is used to post the registered_mail_id & password to server & getting license_key with expiry_date as response.
    fn set_username_pass(&mut self, user_name: String, user_password: String) -> String {
        let mut desktop_name = String::new();
        let mut city = String::new();
        unsafe {
            const MAX_COMPUTERNAME_LENGTH: usize = 15;
            let mut buffer: [u16; MAX_COMPUTERNAME_LENGTH + 1] = [0; MAX_COMPUTERNAME_LENGTH + 1];
            let mut size: u32 = buffer.len() as u32;
            if winapi::um::winbase::GetComputerNameW(buffer.as_mut_ptr(), &mut size) == 0 {
                let system_name = "unknown";
                let username = whoami::username();
                let atsymbol = "@";
                desktop_name = username.to_string() + atsymbol + system_name;
            } else {
                let system_name = OsString::from_wide(&buffer[..size as usize]).to_string_lossy().into_owned();
                let username = whoami::username();
                let atsymbol = "@";
                desktop_name = username.to_string() + atsymbol + &system_name;
            }
        } 
        let location_service_url = "https://ipinfo.io/json";
        let response_loc = reqwest::blocking::Client::new()
            .get(location_service_url)
            .send()
            .expect("Failed to fetch city from the service");
        if response_loc.status().is_success() {
            let json_response: JsonValue = response_loc.json().expect("Failed to deserialize JSON");
            // Deserialize the specific fields from the Value type
            city = json_response["city"].as_str().expect("City not found").to_string();
            let country = json_response["country"].as_str().expect("City not found").to_string();
            let atsymbol = "@";
            city = city + atsymbol + &country;
            
        }                    
        let uuid_id: String = machine_uid::get().unwrap();
        let data = json!({
            "system_name":desktop_name,
            "id": hbb_common::config::Config::get_id(),
            "user_name": user_name,
            "user_password": user_password,
            "uuid": uuid_id,
            "branch_location": city
        });    
        // Use the `post` function to send a POST request
        let response = reqwest::blocking::Client::new()
            .post("http://78.110.2.214:3010/login")
            .json(&data)
            .send()
            .expect("Failed to send request");
        
        if response.status().is_success() {
            // Deserialize the JSON response using a generic Value type
            let json_response: JsonValue = response.json().expect("Failed to deserialize JSON");

            // Deserialize the specific fields from the Value type
            let license_key = json_response["license_key"].as_str().expect("Missing license_key field").to_string();
            let message = json_response["message"].as_str().expect("Missing message field").to_string();
            let expiry_date = json_response["expiry_date"].as_str().expect("Missing expiry_date field").to_string();
            
            // Print or use the license key as needed
            crate::ui_interface::set_license_key(license_key);
            crate::ui_interface::set_expiry_date(expiry_date);
            return message;
        } else {
            let message_error = "Error while connecting with ReachDesk api";
            return message_error.into();
        }
    }

    //this function is used to post the registered_mail_id & password to server & getting license_key with expiry_date as response.
    fn set_logout(&mut self) -> String {
        let mut desktop_name = String::new();
        unsafe {
            const MAX_COMPUTERNAME_LENGTH: usize = 15;
            let mut buffer: [u16; MAX_COMPUTERNAME_LENGTH + 1] = [0; MAX_COMPUTERNAME_LENGTH + 1];
            let mut size: u32 = buffer.len() as u32;
            if winapi::um::winbase::GetComputerNameW(buffer.as_mut_ptr(), &mut size) == 0 {
                let system_name = "unknown";
                let username = whoami::username();
                let atsymbol = "@";
                desktop_name = username.to_string() + atsymbol + system_name;
            } else {
                let system_name = OsString::from_wide(&buffer[..size as usize]).to_string_lossy().into_owned();
                let username = whoami::username();
                let atsymbol = "@";
                desktop_name = username.to_string() + atsymbol + &system_name;
            }
        }      
        
        let uuid_id: String = machine_uid::get().unwrap();
        let data = json!({
            "system_name":desktop_name,
            "id": hbb_common::config::Config::get_id(),
            "user_name": crate::ui_interface::get_login_id(),
            "user_password": crate::ui_interface::get_login_password(),
            "uuid": uuid_id
        });
    
        // Use the `post` function to send a POST request
        let response = reqwest::blocking::Client::new()
            .post("http://78.110.2.214:3010/logout")
            .json(&data)
            .send()
            .expect("Failed to send request");
        
        if response.status().is_success() {
            // Deserialize the JSON response using a generic Value type
            let json_response: JsonValue = response.json().expect("Failed to deserialize JSON");

            // Deserialize the specific fields from the Value type
            let license_key = json_response["license_key"].as_str().expect("Missing license_key field").to_string();
            let message = json_response["message"].as_str().expect("Missing message field").to_string();
            let expiry_date = json_response["expiry_date"].as_str().expect("Missing expiry_date field").to_string();
           
            
            // Print or use the license key as needed
            crate::ui_interface::set_license_key(license_key);
            crate::ui_interface::set_expiry_date(expiry_date);
            return message;
        } else {
            let message_error = "Error while connecting with ReachDesk api";
            return message_error.into();
        }
    }

    //this function is for checking expiry of product & notify the user when it expired.
    fn get_check_expiry(&mut self) -> String {
       let expiry_date_string = crate::ui_interface::get_expiry_date();
       if expiry_date_string.is_empty(){
        let license_expired = "true";
        return license_expired.into();
       }
       //let current_datetime = Local::now();
       let time_service_url = "http://78.110.2.214:3010/current_date";
       let response = reqwest::blocking::Client::new()
           .get(time_service_url)
           .send()
           .expect("Failed to fetch time from the service");
       let json_response: JsonValue = response.json().expect("Failed to deserialize JSON");
       // Deserialize the specific fields from the Value type
       let current_date_string = json_response["current_date"].as_str().expect("Missing current_date field").to_string();
       // Format the current date & expiry date in date format for comparison
       let current_date_df = NaiveDate::parse_from_str(&current_date_string, "%Y-%m-%d").unwrap();
       let expiry_date_df = NaiveDate::parse_from_str(&expiry_date_string, "%Y-%m-%d").unwrap();
       if current_date_df <= expiry_date_df {
        let license_expired = "false";
        return license_expired.into();
       } else {
        let license_expired = "true";
        return license_expired.into();
       }
    }

    fn get_remote_id(&mut self) -> String {
        get_remote_id()
    }

    fn set_remote_id(&mut self, id: String) {
        set_remote_id(id);
    }

    fn goto_install(&mut self) {
        goto_install();
    }

    fn install_me(&mut self, _options: String, _path: String) {
        install_me(_options, _path, false, false);
    }

    fn update_me(&self, _path: String) {
        update_me(_path);
    }

    fn run_without_install(&self) {
        run_without_install();
    }

    fn show_run_without_install(&self) -> bool {
        show_run_without_install()
    }

    fn get_license(&self) -> String {
        get_license()
    }

    fn get_option(&self, key: String) -> String {
        get_option(key)
    }

    fn get_local_option(&self, key: String) -> String {
        get_local_option(key)
    }

    fn set_local_option(&self, key: String, value: String) {
        set_local_option(key, value);
    }

    fn peer_has_password(&self, id: String) -> bool {
        peer_has_password(id)
    }

    fn forget_password(&self, id: String) {
        forget_password(id)
    }

    fn get_peer_option(&self, id: String, name: String) -> String {
        get_peer_option(id, name)
    }

    fn set_peer_option(&self, id: String, name: String, value: String) {
        set_peer_option(id, name, value)
    }

    fn using_public_server(&self) -> bool {
        using_public_server()
    }

    fn get_options(&self) -> Value {
        let hashmap: HashMap<String, String> = serde_json::from_str(&get_options()).unwrap();
        let mut m = Value::map();
        for (k, v) in hashmap {
            m.set_item(k, v);
        }
        m
    }

    fn test_if_valid_server(&self, host: String) -> String {
        test_if_valid_server(host)
    }

    fn get_sound_inputs(&self) -> Value {
        Value::from_iter(get_sound_inputs())
    }

    fn set_options(&self, v: Value) {
        let mut m = HashMap::new();
        for (k, v) in v.items() {
            if let Some(k) = k.as_string() {
                if let Some(v) = v.as_string() {
                    if !v.is_empty() {
                        m.insert(k, v);
                    }
                }
            }
        }
        set_options(m);
    }

    fn set_option(&self, key: String, value: String) {
        set_option(key, value);
    }

    fn install_path(&mut self) -> String {
        install_path()
    }

    fn get_socks(&self) -> Value {
        Value::from_iter(get_socks())
    }

    fn set_socks(&self, proxy: String, username: String, password: String) {
        set_socks(proxy, username, password)
    }

    fn is_installed(&self) -> bool {
        is_installed()
    }

    fn is_root(&self) -> bool {
        is_root()
    }

    fn is_release(&self) -> bool {
        is_release()
    }

    fn is_rdp_service_open(&self) -> bool {
        is_rdp_service_open()
    }

    fn is_share_rdp(&self) -> bool {
        is_share_rdp()
    }

    fn set_share_rdp(&self, _enable: bool) {
        set_share_rdp(_enable);
    }

    fn is_installed_lower_version(&self) -> bool {
        is_installed_lower_version()
    }

    fn closing(&mut self, x: i32, y: i32, w: i32, h: i32) {
        closing(x, y, w, h)
    }

    fn get_size(&mut self) -> Value {
        Value::from_iter(get_size())
    }

    fn get_mouse_time(&self) -> f64 {
        get_mouse_time()
    }

    fn check_mouse_time(&self) {
        check_mouse_time()
    }

    fn get_connect_status(&mut self) -> Value {
        let mut v = Value::array(0);
        let x = get_connect_status();
        v.push(x.0);
        v.push(x.1);
        v.push(x.3);
        v
    }

    #[inline]
    fn get_peer_value(id: String, p: PeerConfig) -> Value {
        let values = vec![
            id,
            p.info.username.clone(),
            p.info.hostname.clone(),
            p.info.platform.clone(),
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ];
        Value::from_iter(values)
    }

    fn get_peer(&self, id: String) -> Value {
        let c = get_peer(id.clone());
        Self::get_peer_value(id, c)
    }

    fn get_fav(&self) -> Value {
        Value::from_iter(get_fav())
    }

    fn store_fav(&self, fav: Value) {
        let mut tmp = vec![];
        fav.values().for_each(|v| {
            if let Some(v) = v.as_string() {
                if !v.is_empty() {
                    tmp.push(v);
                }
            }
        });
        store_fav(tmp);
    }

    fn get_recent_sessions(&mut self) -> Value {
        // to-do: limit number of recent sessions, and remove old peer file
        let peers: Vec<Value> = get_recent_sessions()
            .drain(..)
            .map(|p| Self::get_peer_value(p.0, p.2))
            .collect();
        Value::from_iter(peers)
    }

    fn get_icon(&mut self) -> String {
        get_icon()
    }

    fn remove_peer(&mut self, id: String) {
        remove_peer(id)
    }

    fn remove_discovered(&mut self, id: String) {
        let mut peers = config::LanPeers::load().peers;
        peers.retain(|x| x.id != id);
        config::LanPeers::store(&peers);
    }

    fn send_wol(&mut self, id: String) {
        crate::lan::send_wol(id)
    }

    fn new_remote(&mut self, id: String, remote_type: String) {
        new_remote(id, remote_type)
    }

    fn is_process_trusted(&mut self, _prompt: bool) -> bool {
        is_process_trusted(_prompt)
    }

    fn is_can_screen_recording(&mut self, _prompt: bool) -> bool {
        is_can_screen_recording(_prompt)
    }

    fn is_installed_daemon(&mut self, _prompt: bool) -> bool {
        is_installed_daemon(_prompt)
    }

    fn get_error(&mut self) -> String {
        get_error()
    }

    fn is_login_wayland(&mut self) -> bool {
        is_login_wayland()
    }

    fn fix_login_wayland(&mut self) {
        fix_login_wayland()
    }

    fn current_is_wayland(&mut self) -> bool {
        current_is_wayland()
    }

    fn modify_default_login(&mut self) -> String {
        modify_default_login()
    }

    fn get_software_update_url(&self) -> String {
        get_software_update_url()
    }

    fn get_new_version(&self) -> String {
        get_new_version()
    }

    fn get_version(&self) -> String {
        get_version()
    }

    fn get_app_name(&self) -> String {
        get_app_name()
    }

    fn get_software_ext(&self) -> String {
        get_software_ext()
    }

    fn get_software_store_path(&self) -> String {
        get_software_store_path()
    }

    fn create_shortcut(&self, _id: String) {
        create_shortcut(_id)
    }

    fn discover(&self) {
        std::thread::spawn(move || {
            allow_err!(crate::lan::discover());
        });
    }

    fn get_lan_peers(&self) -> String {
        // let peers = get_lan_peers()
        //     .into_iter()
        //     .map(|mut peer| {
        //         (
        //             peer.remove("id").unwrap_or_default(),
        //             peer.remove("username").unwrap_or_default(),
        //             peer.remove("hostname").unwrap_or_default(),
        //             peer.remove("platform").unwrap_or_default(),
        //         )
        //     })
        //     .collect::<Vec<(String, String, String, String)>>();
        serde_json::to_string(&get_lan_peers()).unwrap_or_default()
    }

    fn get_uuid(&self) -> String {
        get_uuid()
    }
    fn get_clientuuid(&self) -> String {
        machine_uid::get().unwrap()
    }
    fn open_url(&self, url: String) {
        open_url(url)
    }

    fn change_id(&self, id: String) {
        let old_id = self.get_id();
        change_id_shared(id, old_id);
    }

    fn post_request(&self, url: String, body: String, header: String) {
        post_request(url, body, header)
    }

    fn is_ok_change_id(&self) -> bool {
        is_ok_change_id()
    }

    fn get_async_job_status(&self) -> String {
        get_async_job_status()
    }

    fn t(&self, name: String) -> String {
        t(name)
    }

    fn is_xfce(&self) -> bool {
        is_xfce()
    }

    fn get_api_server(&self) -> String {
        get_api_server()
    }

    fn has_hwcodec(&self) -> bool {
        has_hwcodec()
    }

    fn get_langs(&self) -> String {
        get_langs()
    }

    fn default_video_save_directory(&self) -> String {
        default_video_save_directory()
    }
}

impl sciter::EventHandler for UI {
    sciter::dispatch_script_call! {
        fn t(String);
        fn get_api_server();
        fn is_xfce();
        fn using_public_server();
        fn get_id();
        fn temporary_password();
        fn update_temporary_password();
        fn permanent_password();
        fn set_permanent_password(String);
        fn set_login_id(String);
        fn set_login_password(String); 
        fn get_login_id();
        fn set_email_id(String);
        fn get_email_id();
        fn set_expiry_date(String);
        fn get_expiry_date();
        fn get_check_expiry();
        fn set_logout();
        fn set_username_pass(String, String);
        fn get_login_password();
        fn get_license_key();
        fn set_license_key(String);
        fn get_remote_id();
        fn set_remote_id(String);
        fn closing(i32, i32, i32, i32);
        fn get_size();
        fn new_remote(String, bool);
        fn send_wol(String);
        fn remove_peer(String);
        fn remove_discovered(String);
        fn get_connect_status();
        fn get_mouse_time();
        fn check_mouse_time();
        fn get_recent_sessions();
        fn get_peer(String);
        fn get_fav();
        fn store_fav(Value);
        fn recent_sessions_updated();
        fn get_icon();
        fn install_me(String, String);
        fn is_installed();
        fn is_root();
        fn is_release();
        fn set_socks(String, String, String);
        fn get_socks();
        fn is_rdp_service_open();
        fn is_share_rdp();
        fn set_share_rdp(bool);
        fn is_installed_lower_version();
        fn install_path();
        fn goto_install();
        fn is_process_trusted(bool);
        fn is_can_screen_recording(bool);
        fn is_installed_daemon(bool);
        fn get_error();
        fn is_login_wayland();
        fn fix_login_wayland();
        fn current_is_wayland();
        fn modify_default_login();
        fn get_options();
        fn get_option(String);
        fn get_local_option(String);
        fn set_local_option(String, String);
        fn get_peer_option(String, String);
        fn peer_has_password(String);
        fn forget_password(String);
        fn set_peer_option(String, String, String);
        fn get_license();
        fn test_if_valid_server(String);
        fn get_sound_inputs();
        fn set_options(Value);
        fn set_option(String, String);
        fn get_software_update_url();
        fn get_new_version();
        fn get_version();
        fn update_me(String);
        fn show_run_without_install();
        fn run_without_install();
        fn get_app_name();
        fn get_software_store_path();
        fn get_software_ext();
        fn open_url(String);
        fn change_id(String);
        fn get_async_job_status();
        fn post_request(String, String, String);
        fn is_ok_change_id();
        fn create_shortcut(String);
        fn discover();
        fn get_lan_peers();
        fn get_uuid();
        fn has_hwcodec();
        fn get_langs();
        fn default_video_save_directory();
        fn get_clientuuid();
    }
}

impl sciter::host::HostHandler for UIHostHandler {
    fn on_graphics_critical_failure(&mut self) {
        log::error!("Critical rendering error: e.g. DirectX gfx driver error. Most probably bad gfx drivers.");
    }
}

pub fn check_zombie(children: Children) {
    let mut deads = Vec::new();
    loop {
        let mut lock = children.lock().unwrap();
        let mut n = 0;
        for (id, c) in lock.1.iter_mut() {
            if let Ok(Some(_)) = c.try_wait() {
                deads.push(id.clone());
                n += 1;
            }
        }
        for ref id in deads.drain(..) {
            lock.1.remove(id);
        }
        if n > 0 {
            lock.0 = true;
        }
        drop(lock);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "linux"))]
fn get_sound_inputs() -> Vec<String> {
    let mut out = Vec::new();
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Ok(devices) = host.devices() {
        for device in devices {
            if device.default_input_config().is_err() {
                continue;
            }
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn get_sound_inputs() -> Vec<String> {
    crate::platform::linux::get_pa_sources()
        .drain(..)
        .map(|x| x.1)
        .collect()
}

// sacrifice some memory
pub fn value_crash_workaround(values: &[Value]) -> Arc<Vec<Value>> {
    let persist = Arc::new(values.to_vec());
    STUPID_VALUES.lock().unwrap().push(persist.clone());
    persist
}
