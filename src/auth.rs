use serde::{Deserialize, Serialize};

use crate::store::user_store::{User, UserRole};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user is disabled")]
    UserDisabled,
    #[error("password hash error: {0}")]
    HashError(String),
    #[error("store error: {0}")]
    StoreError(String),
}

/// Hash a password using argon2 with a random salt.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::HashError(e.to_string()))
}

/// Verify a password against an argon2 PHC hash string.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
    use argon2::password_hash::PasswordVerifier;
    use argon2::Argon2;

    let parsed =
        argon2::PasswordHash::new(hash).map_err(|e| AuthError::HashError(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

// ----------------------------------------------------------------
// Granular permissions
// ----------------------------------------------------------------

/// Individual permission that can be granted or denied per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    WritePoints,
    AcknowledgeAlarms,
    ManageSchedules,
    ManageDiscovery,
    ManagePrograms,
    ManageVirtualPoints,
    ManageUsers,
}

impl Permission {
    pub fn all() -> &'static [Permission] {
        &[
            Self::WritePoints,
            Self::AcknowledgeAlarms,
            Self::ManageSchedules,
            Self::ManageDiscovery,
            Self::ManagePrograms,
            Self::ManageVirtualPoints,
            Self::ManageUsers,
        ]
    }

    pub fn key(&self) -> &'static str {
        match self {
            Self::WritePoints => "write_points",
            Self::AcknowledgeAlarms => "acknowledge_alarms",
            Self::ManageSchedules => "manage_schedules",
            Self::ManageDiscovery => "manage_discovery",
            Self::ManagePrograms => "manage_programs",
            Self::ManageVirtualPoints => "manage_virtual_points",
            Self::ManageUsers => "manage_users",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::WritePoints => "Write Points",
            Self::AcknowledgeAlarms => "Acknowledge Alarms",
            Self::ManageSchedules => "Manage Schedules",
            Self::ManageDiscovery => "Manage Discovery",
            Self::ManagePrograms => "Manage Programs",
            Self::ManageVirtualPoints => "Manage Virtual Points",
            Self::ManageUsers => "Manage Users",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::WritePoints => "Write values to device points (BACnet, Modbus)",
            Self::AcknowledgeAlarms => "Acknowledge active alarms",
            Self::ManageSchedules => "Create, edit, and delete schedules",
            Self::ManageDiscovery => "Accept or ignore discovered devices",
            Self::ManagePrograms => "Create, edit, and delete logic programs",
            Self::ManageVirtualPoints => "Create and write virtual points",
            Self::ManageUsers => "Create, edit, and delete user accounts",
        }
    }

    pub fn from_key(key: &str) -> Option<Permission> {
        match key {
            "write_points" => Some(Self::WritePoints),
            "acknowledge_alarms" => Some(Self::AcknowledgeAlarms),
            "manage_schedules" => Some(Self::ManageSchedules),
            "manage_discovery" => Some(Self::ManageDiscovery),
            "manage_programs" => Some(Self::ManagePrograms),
            "manage_virtual_points" => Some(Self::ManageVirtualPoints),
            "manage_users" => Some(Self::ManageUsers),
            _ => None,
        }
    }
}

/// Permissions assigned to a single role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RolePermissions {
    pub role: UserRole,
    pub write_points: bool,
    pub acknowledge_alarms: bool,
    pub manage_schedules: bool,
    pub manage_discovery: bool,
    pub manage_programs: bool,
    pub manage_virtual_points: bool,
    pub manage_users: bool,
}

impl RolePermissions {
    /// Default permissions for a role.
    pub fn defaults(role: &UserRole) -> Self {
        match role {
            UserRole::Admin => Self {
                role: role.clone(),
                write_points: true,
                acknowledge_alarms: true,
                manage_schedules: true,
                manage_discovery: true,
                manage_programs: true,
                manage_virtual_points: true,
                manage_users: true,
            },
            UserRole::Operator => Self {
                role: role.clone(),
                write_points: true,
                acknowledge_alarms: true,
                manage_schedules: true,
                manage_discovery: false,
                manage_programs: false,
                manage_virtual_points: false,
                manage_users: false,
            },
            UserRole::Viewer => Self {
                role: role.clone(),
                write_points: false,
                acknowledge_alarms: false,
                manage_schedules: false,
                manage_discovery: false,
                manage_programs: false,
                manage_virtual_points: false,
                manage_users: false,
            },
        }
    }

    /// Get the value of a specific permission.
    pub fn get(&self, perm: Permission) -> bool {
        match perm {
            Permission::WritePoints => self.write_points,
            Permission::AcknowledgeAlarms => self.acknowledge_alarms,
            Permission::ManageSchedules => self.manage_schedules,
            Permission::ManageDiscovery => self.manage_discovery,
            Permission::ManagePrograms => self.manage_programs,
            Permission::ManageVirtualPoints => self.manage_virtual_points,
            Permission::ManageUsers => self.manage_users,
        }
    }

    /// Set the value of a specific permission.
    pub fn set(&mut self, perm: Permission, value: bool) {
        match perm {
            Permission::WritePoints => self.write_points = value,
            Permission::AcknowledgeAlarms => self.acknowledge_alarms = value,
            Permission::ManageSchedules => self.manage_schedules = value,
            Permission::ManageDiscovery => self.manage_discovery = value,
            Permission::ManagePrograms => self.manage_programs = value,
            Permission::ManageVirtualPoints => self.manage_virtual_points = value,
            Permission::ManageUsers => self.manage_users = value,
        }
    }
}

/// All three roles' permissions together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllRolePermissions {
    pub admin: RolePermissions,
    pub operator: RolePermissions,
    pub viewer: RolePermissions,
}

impl Default for AllRolePermissions {
    fn default() -> Self {
        Self {
            admin: RolePermissions::defaults(&UserRole::Admin),
            operator: RolePermissions::defaults(&UserRole::Operator),
            viewer: RolePermissions::defaults(&UserRole::Viewer),
        }
    }
}

impl AllRolePermissions {
    pub fn for_role(&self, role: &UserRole) -> &RolePermissions {
        match role {
            UserRole::Admin => &self.admin,
            UserRole::Operator => &self.operator,
            UserRole::Viewer => &self.viewer,
        }
    }

    pub fn for_role_mut(&mut self, role: &UserRole) -> &mut RolePermissions {
        match role {
            UserRole::Admin => &mut self.admin,
            UserRole::Operator => &mut self.operator,
            UserRole::Viewer => &mut self.viewer,
        }
    }
}

/// Check if a user has a specific permission, given the current role permissions config.
pub fn has_permission(user: &User, perm: Permission, all_perms: &AllRolePermissions) -> bool {
    all_perms.for_role(&user.role).get(perm)
}

// Legacy helpers — kept for backward compat, now just check the hardcoded defaults.
// GUI code should prefer has_permission() with the stored AllRolePermissions.

/// Returns true if the user has write permissions (Admin or Operator) by default.
pub fn can_write(user: &User) -> bool {
    matches!(user.role, UserRole::Admin | UserRole::Operator)
}

/// Returns true if the user has admin permissions by default.
pub fn can_admin(user: &User) -> bool {
    matches!(user.role, UserRole::Admin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let hash = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn role_checks() {
        let admin = User {
            id: "1".into(),
            username: "admin".into(),
            display_name: "Admin".into(),
            role: UserRole::Admin,
            password_hash: String::new(),
            created_ms: 0,
            last_login_ms: None,
            disabled: false,
        };
        assert!(can_write(&admin));
        assert!(can_admin(&admin));

        let operator = User {
            role: UserRole::Operator,
            ..admin.clone()
        };
        assert!(can_write(&operator));
        assert!(!can_admin(&operator));

        let viewer = User {
            role: UserRole::Viewer,
            ..admin.clone()
        };
        assert!(!can_write(&viewer));
        assert!(!can_admin(&viewer));
    }

    #[test]
    fn default_permissions() {
        let all = AllRolePermissions::default();
        let admin = User {
            id: "1".into(),
            username: "admin".into(),
            display_name: "Admin".into(),
            role: UserRole::Admin,
            password_hash: String::new(),
            created_ms: 0,
            last_login_ms: None,
            disabled: false,
        };
        assert!(has_permission(&admin, Permission::WritePoints, &all));
        assert!(has_permission(&admin, Permission::ManageUsers, &all));

        let op = User { role: UserRole::Operator, ..admin.clone() };
        assert!(has_permission(&op, Permission::WritePoints, &all));
        assert!(!has_permission(&op, Permission::ManageUsers, &all));

        let viewer = User { role: UserRole::Viewer, ..admin.clone() };
        assert!(!has_permission(&viewer, Permission::WritePoints, &all));
        assert!(!has_permission(&viewer, Permission::ManageUsers, &all));
    }

    #[test]
    fn custom_permissions() {
        let mut all = AllRolePermissions::default();
        // Grant viewer write_points
        all.viewer.write_points = true;
        // Revoke operator manage_schedules
        all.operator.manage_schedules = false;

        let viewer = User {
            id: "1".into(),
            username: "v".into(),
            display_name: "V".into(),
            role: UserRole::Viewer,
            password_hash: String::new(),
            created_ms: 0,
            last_login_ms: None,
            disabled: false,
        };
        assert!(has_permission(&viewer, Permission::WritePoints, &all));

        let op = User { role: UserRole::Operator, ..viewer.clone() };
        assert!(!has_permission(&op, Permission::ManageSchedules, &all));
    }
}
