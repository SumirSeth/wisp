import re
"""Offline integration: tagged 0.8.2 updater + current native window/update sources.
The hidden fixture host replaces dock startup, so global hotkeys/taskbar are untouched.
"""
import hashlib, json, os, shutil, subprocess, tempfile, time
from pathlib import Path
REPO=Path(__file__).resolve().parents[1]
TARGET=REPO/'target'
def wait(test, message, seconds=60):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if test(): return
        time.sleep(.1)
    raise AssertionError(message)
def main():
    root=Path(tempfile.mkdtemp(prefix='wisp-update-migration-'))
    print(f'Evidence: {root}',flush=True)
    log=(root/'build.log').open('w')
    manifest=(REPO/'Cargo.toml').read_text().replace('name = "wisp"','name = "update-fixture"',1)
    manifest=manifest[:manifest.index('[profile.release]')]
    manifest+='\n[[bin]]\nname = "update-fixture"\npath = "main.rs"\n'
    fixture=root/'source';fixture.mkdir()
    source=(REPO/'examples/launcher_demo.rs').read_text().split('mod channel {')[0]
    source=source.replace('../src/',(REPO/'src').as_posix()+'/')
    source+=r'''
mod channel {
 use windows::core::{PCWSTR,w};
 pub const NIGHTLY:bool=false; pub const NAME:&str="Wisp update fixture"; pub const DATA_DIR:&str="wisp-fixture";
 pub const LAUNCHER_CLASS:PCWSTR=w!("Wisp.UpdateFixture.Launcher");
 pub const SETTINGS_CLASS:PCWSTR=w!("Wisp.UpdateFixture.Settings");
 pub const INSTANCE_MUTEX:PCWSTR=w!("Local\\Wisp.UpdateFixture.Instance");
 pub const RUN_VALUE:PCWSTR=w!("Wisp fixture unused"); pub const NATIVE_SETTINGS_MESSAGE:u32=0x800a;
}
use windows::{core::w,Win32::{Foundation::*,UI::WindowsAndMessaging::*,System::LibraryLoader::GetModuleHandleW}};
fn marker(name:&str)->std::path::PathBuf{std::env::current_exe().unwrap().parent().unwrap().join(name)}
unsafe extern "system" fn host(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM)->LRESULT{
 match msg {
  0x8004=>{unsafe{let _=DestroyWindow(hwnd);}},
  0x800a=>{native_settings::open(wp.0).unwrap();std::fs::write(marker("settings-opened"),wp.0.to_string()).unwrap();},
  WM_TIMER=>{
   if marker("quit-host").exists(){unsafe{let _=DestroyWindow(hwnd);}}
   else if marker("update-now").exists() && env!("CARGO_PKG_VERSION")=="0.9.0" {
    let _=std::fs::remove_file(marker("update-now"));
    let release=update::check().unwrap().unwrap();let raw=hwnd.0 as usize;
    update::set_worker(std::thread::spawn(move||update::install(&release,move|result|{
     result.unwrap();unsafe{let _=PostMessageW(Some(HWND(raw as _)),0x8004,WPARAM(0),LPARAM(0));}
    })));
   }
  },
  WM_DESTROY=>unsafe{PostQuitMessage(0);},
  _=>return unsafe{DefWindowProcW(hwnd,msg,wp,lp)}
 } LRESULT(0)
}
fn main(){
 let args:Vec<String>=std::env::args().collect();if migration::dispatch(&args).unwrap(){return;}
 if !args.iter().any(|a|a=="--old-host"){update::remove_old_files();}
 unsafe{
  let instance=GetModuleHandleW(None).unwrap();
  RegisterClassW(&WNDCLASSW{lpfnWndProc:Some(host),hInstance:instance.into(),lpszClassName:channel::LAUNCHER_CLASS,..Default::default()});
  let hwnd=CreateWindowExW(WINDOW_EX_STYLE::default(),channel::LAUNCHER_CLASS,w!("fixture"),WS_OVERLAPPED,0,0,0,0,None,None,Some(instance.into()),None).unwrap();
  SetTimer(Some(hwnd),1,100,None);
  std::fs::write(marker(&format!("started-{}",env!("CARGO_PKG_VERSION"))),std::process::id().to_string()).unwrap();
  let mut msg=MSG::default();while GetMessageW(&mut msg,None,0,0).0>0{let _=TranslateMessage(&msg);DispatchMessageW(&msg);}
 }update::finish_restart();
}
'''
    (fixture/'main.rs').write_text(source)
    shutil.copy2(REPO/'Cargo.lock',fixture/'Cargo.lock')
    def build(version):
        import re
        text=re.sub(r'version = "0\.[0-9]+\.[0-9]+"',f'version = "{version}"',manifest,count=1)
        (fixture/'Cargo.toml').write_text(text)
        subprocess.run(['cargo','build','--manifest-path',str(fixture/'Cargo.toml'),'--target-dir',str(TARGET)],stdout=log,stderr=log,check=True)
        exe=root/f'fixture-{version}.exe';shutil.copy2(TARGET/'debug/update-fixture.exe',exe);return exe
    exe090=build('0.9.0');exe091=build('0.9.1')
    old=root/'old-source';old.mkdir()
    tagged=subprocess.check_output(['git','show','v0.8.2:settings/src/update.rs'],cwd=REPO,text=True)
    tagged=tagged.replace('fn get(url: &str) -> Result<Vec<u8>, String> {','fn get(url: &str) -> Result<Vec<u8>, String> {\n if let Some(path)=url.strip_prefix("file://"){return std::fs::read(path).map_err(|e|e.to_string());}')
    (old/'update.rs').write_text(tagged)
    text=manifest.replace('update-fixture','old-updater').replace('[dependencies]','[dependencies]\nzip = {version="2",default-features=false,features=["deflate"]}')
    text=re.sub(r'version = "0\.[0-9]+\.[0-9]+"','version = "0.8.2"',text,count=1)
    (old/'Cargo.toml').write_text(text)
    shutil.copy2(REPO/'Cargo.lock',old/'Cargo.lock')
    source=r'''
#![allow(dead_code)]
mod update;
mod channel{use windows::core::{PCWSTR,w};pub const NIGHTLY:bool=false;pub const LAUNCHER_CLASS:PCWSTR=w!("Wisp.UpdateFixture.Launcher");}
mod config{
 pub fn path()->Result<std::path::PathBuf,String>{Err("unused".into())}
 pub struct Config{pub check_for_updates:bool}pub struct Store;
 impl Store{pub fn new(_:std::path::PathBuf)->Self{Self}pub fn load(&self)->Result<Config,String>{Err("unused".into())}}
}
'''
    for module in ['install','update_notice']:
        source+=f'#[path="{(REPO/"src"/(module+".rs")).as_posix()}"]mod {module};\n'
    source+=r'''
fn main(){
 let dir=std::env::current_exe().unwrap().parent().unwrap().to_owned();
 let mut launcher=std::process::Command::new(dir.join("wisp.exe")).arg("--old-host").spawn().unwrap();
 std::thread::sleep(std::time::Duration::from_secs(1));
 update::install(&update::check().unwrap().unwrap()).unwrap();let _=launcher.wait();
 std::fs::write(dir.join("old-installed"),"ok").unwrap();
 while !dir.join("quit-settings").exists(){std::thread::sleep(std::time::Duration::from_millis(100));}
}
'''
    (old/'main.rs').write_text(source)
    oldtarget=TARGET/'old-082-updater'
    subprocess.run(['cargo','build','--manifest-path',str(old/'Cargo.toml'),'--target-dir',str(oldtarget)],stdout=log,stderr=log,check=True)
    install=root/'install';install.mkdir();shutil.copy2(exe090,install/'wisp.exe');shutil.copy2(oldtarget/'debug/old-updater.exe',install/'wisp-settings.exe')
    release=root/'release090'
    subprocess.run([shutil.which('pwsh') or 'powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(REPO/'scripts/package-release.ps1'),'-ExePath',str(exe090),'-Tag','v0.9.0','-OutputDirectory',str(release)],stdout=log,stderr=log,check=True)
    def release_json(folder,version):
        data={'tag_name':f'v{version}','assets':[{'name':p.name,'browser_download_url':'file://'+str(p)} for p in folder.iterdir() if p.is_file()]}
        path=folder/'release.json';path.write_text(json.dumps(data));return path
    env=os.environ.copy();env['APPDATA']=str(root/'appdata');env['WISP_RELEASE_URL']='file://'+str(release_json(release,'0.9.0'))
    data=Path(env['APPDATA'])/'wisp-fixture';data.mkdir(parents=True);shutil.copy2(REPO/'src/default-config.toml',data/'config.toml')
    proc=subprocess.Popen([str(install/'wisp-settings.exe')],env=env,stdout=log,stderr=log)
    try:
        wait(lambda:(install/'old-installed').exists(),'0.8.2 install failed')
        assert (install/'wisp.exe').read_bytes()==exe090.read_bytes()
        assert (install/'wisp-settings.exe').read_bytes()==exe090.read_bytes()
        for args,page in [(['--page','about'],'3'),(['--welcome'],'4'),(['--check-updates'],'0')]:
            (install/'settings-opened').unlink(missing_ok=True)
            alias=subprocess.Popen([str(install/'wisp-settings.exe')]+args,env=env,stdout=log,stderr=log)
            alias.wait(timeout=10);assert alias.returncode==0
            wait(lambda:(install/'settings-opened').exists(),'native Settings did not open')
            assert (install/'settings-opened').read_text()==page
        assert (install/'wisp-settings.exe').exists(),'Restart Settings removed while old app runs'
        (install/'quit-settings').touch();proc.wait(timeout=10)
        leftovers=['wisp-settings.exe','wisp-settings.exe.old','wisp.exe.old']
        # The old About window may outlive the brief startup retry. Exercise the
        # specified next-start cleanup path as well, without touching a real dock.
        if any((install/n).exists() for n in leftovers):
            pid=int((install/'started-0.9.0').read_text())
            (install/'quit-host').touch()
            subprocess.run(['powershell','-NoProfile','-Command',f'Wait-Process -Id {pid} -Timeout 15 -ErrorAction SilentlyContinue'],stdout=log,stderr=log)
            (install/'quit-host').unlink()
            subprocess.Popen([str(install/'wisp.exe')],env=env,stdout=log,stderr=log)
        wait(lambda:all(not(install/n).exists() for n in leftovers),'legacy cleanup failed')
        release1=root/'release091';release1.mkdir();name='wisp-v0.9.1-x86_64.exe';shutil.copy2(exe091,release1/name)
        (release1/'SHA256SUMS.txt').write_text(hashlib.sha256(exe091.read_bytes()).hexdigest()+'  '+name+'\n')
        (release/'release.json').write_text(release_json(release1,'0.9.1').read_text());(install/'update-now').touch()
        wait(lambda:(install/'started-0.9.1').exists(),'single-exe restart failed')
        assert (install/'wisp.exe').read_bytes()==exe091.read_bytes()
        wait(lambda:not(install/'wisp.exe.old').exists(),'new backup cleanup failed')
        result='PASS: v0.8.2 install, both swaps, restart, native About/welcome/hidden-check forwarding, deferred and next-start cleanup, 0.9.0 to 0.9.1 single-exe swap/restart/cleanup.\n'
        (root/'result.txt').write_text(result);print(result,flush=True)
    finally:
        (install/'quit-settings').touch();(install/'quit-host').touch()
        if proc.poll() is None:proc.wait(timeout=10)
if __name__=='__main__':main()
