//! Global query-scope application and bypass modifiers on [`QueryBuilder`].
//!
//! Global scopes attached via [`QueryBuilder::with_global_scopes`] are applied
//! lazily: [`QueryBuilder::to_sql`] (and the executor methods) resolve a copy of
//! the builder with every active scope applied, so `bindings()` and the emitted
//! `$n` placeholders always agree. Bypass a scope for one query with
//! [`QueryBuilder::without_global_scopes`] (all) or
//! [`QueryBuilder::without_global_scope::<S>`] (one, by type).

use super::{Condition, QueryBuilder};
use crate::scopes::{GlobalScopeEntry, SoftDeletesScope};
use std::any::TypeId;

impl QueryBuilder {
    /// Attach global scopes to this query.
    ///
    /// The scopes are applied when the SQL is rendered (or executed), never at
    /// attach time, so a later bypass modifier still takes effect.
    pub fn with_global_scopes(mut self, scopes: Vec<GlobalScopeEntry>) -> Self {
        self.global_scopes = scopes;
        self.global_scopes_applied = false;
        self.global_scopes_bypassed = false;
        self
    }

    /// The global scopes still active on this query (after any bypasses).
    pub fn global_scopes(&self) -> &[GlobalScopeEntry] {
        &self.global_scopes
    }

    /// Whether `without_global_scopes()` disabled every global scope.
    pub fn global_scopes_bypassed(&self) -> bool {
        self.global_scopes_bypassed
    }

    /// Whether a global scope of concrete type `S` is active on this query.
    pub fn has_global_scope<S: 'static>(&self) -> bool {
        let type_id = TypeId::of::<S>();
        !self.global_scopes_bypassed
            && !self.bypassed_scopes.contains(&type_id)
            && self
                .global_scopes
                .iter()
                .any(|entry| entry.type_id() == type_id)
    }

    /// Bypass every global scope for this query (`withoutGlobalScopes`).
    ///
    /// Removes the attached scopes and strips any scope-owned conditions already
    /// present in the builder (e.g. a `deleted_at IS NULL` guard applied earlier
    /// via [`QueryBuilder::with_soft_deletes`]), so a pre-resolved or partially
    /// built query is fully unscoped. Caller-authored conditions are preserved.
    pub fn without_global_scopes(mut self) -> Self {
        self.global_scopes.clear();
        self.global_scopes_bypassed = true;
        self.global_scopes_applied = false;
        self.conditions
            .retain(|condition| condition.scope_type.is_none());
        self.soft_delete_guard = Some(true);
        self
    }

    /// Bypass the global scope of concrete type `S` for this query
    /// (`withoutGlobalScope(SoftDeletesScope::class)`).
    ///
    /// Also strips conditions already tagged with `S`'s [`TypeId`] so a guard
    /// applied before the bypass (e.g. via [`QueryBuilder::with_soft_deletes`])
    /// no longer constrains the query. Bypassing [`SoftDeletesScope`] records the
    /// trashed state so a later scope application does not re-add the guard.
    pub fn without_global_scope<S: 'static>(mut self) -> Self {
        let type_id = TypeId::of::<S>();
        self.global_scopes
            .retain(|entry| entry.type_id() != type_id);
        if !self.bypassed_scopes.contains(&type_id) {
            self.bypassed_scopes.push(type_id);
        }
        self.conditions
            .retain(|condition| condition.scope_type != Some(type_id));
        if type_id == TypeId::of::<SoftDeletesScope>() {
            self.soft_delete_guard = Some(true);
        }
        self.global_scopes_applied = false;
        self
    }

    /// A copy of this builder with every active global scope applied.
    ///
    /// Idempotent: a builder already carrying its scopes is returned unchanged.
    /// Callers pair this with `bindings()` so the SQL and bind values stay in
    /// step.
    pub fn resolved(&self) -> QueryBuilder {
        let mut builder = self.clone();
        builder.apply_global_scopes();
        builder
    }

    /// Apply the active global scopes in place, tagging their conditions.
    fn apply_global_scopes(&mut self) {
        if self.global_scopes_applied || self.global_scopes_bypassed {
            self.global_scopes_applied = true;
            return;
        }
        let scopes = self.global_scopes.clone();
        for entry in &scopes {
            if self.bypassed_scopes.contains(&entry.type_id()) {
                continue;
            }
            self.current_scope = Some(entry.type_id());
            entry.apply(self);
        }
        self.current_scope = None;
        self.global_scopes_applied = true;
    }

    /// Add the `deleted_at IS NULL` guard for [`SoftDeletesScope`], idempotently.
    ///
    /// A no-op when trashed rows were explicitly requested (`with_trashed`) or
    /// the guard is already present. Called by [`SoftDeletesScope::apply`] so the
    /// soft-delete filter participates in the global-scope bypass machinery.
    pub(crate) fn apply_soft_delete_guard(&mut self) {
        if self.soft_delete_guard == Some(true) {
            return;
        }
        if self
            .conditions
            .iter()
            .any(|condition| condition.sql == "deleted_at IS NULL")
        {
            self.soft_delete_guard = Some(false);
            return;
        }
        let mut condition = Condition::new("AND", "deleted_at IS NULL");
        condition.scope_type = Some(TypeId::of::<SoftDeletesScope>());
        self.conditions.push(condition);
        self.soft_delete_guard = Some(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::{CallbackScope, GlobalScope};
    use crate::types::Value;

    /// A named global scope used to exercise type-based bypass.
    struct TenantScope;

    impl GlobalScope for TenantScope {
        fn id(&self) -> &'static str {
            "tenant"
        }

        fn apply(&self, builder: &mut QueryBuilder) {
            *builder = std::mem::take(builder).where_eq("tenant_id", Value::Int(7));
        }
    }

    /// TASK-027: a guard already applied via `with_soft_deletes(true)` is removed
    /// by `without_global_scope::<SoftDeletesScope>()`.
    #[test]
    fn bypass_removes_preapplied_soft_delete_condition() {
        let qb = QueryBuilder::table("users")
            .with_soft_deletes(true)
            .without_global_scope::<SoftDeletesScope>();
        assert_eq!(qb.to_sql().unwrap(), "SELECT * FROM users");
    }

    /// TASK-027: `without_global_scopes()` strips every scope-owned condition but
    /// keeps caller-authored ones.
    #[test]
    fn bypass_all_strips_only_scope_conditions() {
        let qb = QueryBuilder::table("users")
            .with_soft_deletes(true)
            .where_eq("name", "Ada")
            .without_global_scopes();
        assert_eq!(qb.to_sql().unwrap(), "SELECT * FROM users WHERE name = $1");
    }

    /// TASK-027: a non-soft-delete scope's condition is tagged and removed by its
    /// concrete type via `without_global_scope::<S>()`.
    #[test]
    fn bypass_removes_typed_scope_condition() {
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(TenantScope)])
            .where_eq("name", "Ada");
        assert!(
            qb.to_sql().unwrap().contains("tenant_id = $2"),
            "{}",
            qb.to_sql().unwrap()
        );

        let bypassed = qb.without_global_scope::<TenantScope>();
        assert_eq!(
            bypassed.to_sql().unwrap(),
            "SELECT * FROM users WHERE name = $1"
        );
    }

    /// TASK-027: callback scopes participate in the same tagging/bypass flow.
    #[test]
    fn bypass_removes_callback_scope_condition() {
        let scope = CallbackScope::new("callback", |b| {
            *b = std::mem::take(b).where_eq("flag", true);
        });
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(scope)])
            .where_eq("name", "Ada");
        let stripped = qb.without_global_scopes();
        assert_eq!(
            stripped.to_sql().unwrap(),
            "SELECT * FROM users WHERE name = $1"
        );
    }

    /// TASK-028: user `OR` conditions are parenthesized before the scope guard is
    /// appended so soft-deleted rows cannot leak.
    #[test]
    fn or_conditions_are_grouped_before_scope_guard() {
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(SoftDeletesScope)])
            .where_eq("a", 1)
            .or_where_eq("b", 2);
        assert_eq!(
            qb.to_sql().unwrap(),
            "SELECT * FROM users WHERE (a = $1 OR b = $2) AND deleted_at IS NULL"
        );
    }

    /// TASK-028: a pure-AND predicate is unchanged (no redundant parentheses).
    #[test]
    fn and_only_conditions_are_not_grouped() {
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(SoftDeletesScope)])
            .where_eq("a", 1)
            .where_eq("b", 2);
        assert_eq!(
            qb.to_sql().unwrap(),
            "SELECT * FROM users WHERE a = $1 AND b = $2 AND deleted_at IS NULL"
        );
    }

    /// TASK-029: `resolved_bindings()` includes scope-contributed values so the
    /// `$n` placeholders in `to_sql()` and the bind list stay aligned.
    #[test]
    fn resolved_bindings_include_scope_values() {
        let scope = CallbackScope::new("tenant", |b| {
            *b = std::mem::take(b).where_eq("tenant_id", Value::Int(7));
        });
        let qb = QueryBuilder::table("users")
            .with_global_scopes(vec![GlobalScopeEntry::new(scope)])
            .where_eq("name", "Ada");

        // The unresolved builder only knows its own binding.
        assert_eq!(qb.bindings(), &[Value::Text("Ada".into())]);

        let sql = qb.to_sql().unwrap();
        let bindings = qb.resolved_bindings();
        assert_eq!(bindings, vec![Value::Text("Ada".into()), Value::Int(7)]);
        assert!(sql.contains("name = $1"), "{sql}");
        assert!(sql.contains("tenant_id = $2"), "{sql}");
        assert_eq!(bindings.len(), 2);
    }
}
