//! What is playing, and play, pause, next and previous, through the system
//! media controls that Windows shows in its volume flyout.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub title: String,
    pub artist: String,
    /// The app that plays, such as "Spotify".
    pub app: String,
    pub playing: bool,
}

fn session() -> windows::core::Result<Session> {
    Manager::RequestAsync()?.join()?.GetCurrentSession()
}

/// `None` if nothing is playing or paused.
pub fn state() -> Option<State> {
    let session = session().ok()?;
    let properties = session.TryGetMediaPropertiesAsync().and_then(|op| op.join()).ok();
    let text = |value: windows::core::Result<windows::core::HSTRING>| value.map(|v| v.to_string()).unwrap_or_default();
    Some(State {
        title: properties.as_ref().map(|p| text(p.Title())).unwrap_or_default(),
        artist: properties.as_ref().map(|p| text(p.Artist())).unwrap_or_default(),
        app: app_name(&text(session.SourceAppUserModelId())),
        playing: session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .is_ok_and(|status| status == Status::Playing),
    })
}

pub fn control(control: Control) -> Result<(), String> {
    let session = session().map_err(|_| "Nothing is playing.".to_string())?;
    let request = match control {
        Control::PlayPause => session.TryTogglePlayPauseAsync(),
        Control::Play => session.TryPlayAsync(),
        Control::Pause => session.TryPauseAsync(),
        Control::Next => session.TrySkipNextAsync(),
        Control::Previous => session.TrySkipPreviousAsync(),
    };
    match request.and_then(|op| op.join()) {
        Ok(true) => Ok(()),
        Ok(false) => Err("The app that is playing did not accept the request.".into()),
        Err(error) => Err(error.message()),
    }
}

/// "Spotify.exe" → "Spotify"; "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" → "ZuneMusic".
fn app_name(id: &str) -> String {
    let base = id.rsplit(['!', '\\']).next().unwrap_or(id);
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    let base = base.rsplit('.').next().unwrap_or(base);
    match base.to_lowercase().as_str() {
        "msedge" => "Microsoft Edge".into(),
        "chrome" => "Google Chrome".into(),
        _ => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_apps_from_their_ids() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(
            app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "ZuneMusic"
        );
        assert_eq!(app_name("MSEdge"), "Microsoft Edge");
        assert_eq!(app_name(r"C:\Apps\helium.exe"), "helium");
    }
}
