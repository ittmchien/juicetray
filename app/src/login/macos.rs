use anyhow::{Result, anyhow};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use super::LoginState;
use crate::instance;

pub fn login_state() -> LoginState {
    // The login-item API only knows apps launched from a bundle.
    if !instance::is_bundled() {
        return LoginState::Unavailable;
    }
    // SAFETY: plain getters on the main-app service object.
    let status = unsafe { SMAppService::mainAppService().status() };
    match status {
        SMAppServiceStatus::Enabled => LoginState::Enabled,
        // A main app never registered reports NotFound, not NotRegistered; both can register.
        SMAppServiceStatus::NotRegistered | SMAppServiceStatus::NotFound => LoginState::Disabled,
        SMAppServiceStatus::RequiresApproval => LoginState::RequiresApproval,
        _ => LoginState::Unavailable,
    }
}

/// Registers or removes the login item; a pending approval opens System Settings instead.
pub fn set_start_at_login(enabled: bool) -> Result<()> {
    if enabled && login_state() == LoginState::RequiresApproval {
        // SAFETY: class method without arguments.
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        return Ok(());
    }
    // SAFETY: register/unregister only touch this app's own login item.
    let result = unsafe {
        let service = SMAppService::mainAppService();
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|error| anyhow!("{}", error.localizedDescription()))
}
