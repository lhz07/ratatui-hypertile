mod types;

pub use types::{PluginContext, RegistryError};

use ratatui::{buffer::Buffer, layout::Rect};
use ratatui_hypertile::{EventOutcome, HypertileEvent, PaneId};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Trait implemented by pane-local plugins stored in [`Registry`].
pub trait HypertilePlugin {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool, target_rect: Option<Rect>);

    /// Return [`EventOutcome::Consumed`] to mark it handled.
    fn on_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        let _ = event;
        EventOutcome::Ignored
    }
    // TODO: make pane id globally unique, and use it to decide whether to send render msg
    fn on_mount(&mut self, ctx: PluginContext) {
        let _ = ctx;
    }

    fn on_unmount(&mut self, ctx: PluginContext) {
        let _ = ctx;
    }

    fn is_closed(&mut self) -> bool {
        false
    }

    fn on_active_change(&mut self, active: bool) {
        let _ = active;
    }
}

pub struct PluginInstance {
    plugin_type: String,
    plugin: Box<dyn HypertilePlugin>,
}

impl PluginInstance {
    pub fn plugin_type(&self) -> &str {
        &self.plugin_type
    }
    pub fn plugin(&mut self) -> &mut Box<dyn HypertilePlugin> {
        &mut self.plugin
    }
}

/// Stores plugin factories and mounted plugin instances keyed by pane id.
#[derive(Default)]
pub struct Registry {
    factories: BTreeMap<String, Box<dyn Fn() -> Box<dyn HypertilePlugin>>>,
    instances: HashMap<PaneId, PluginInstance>,
}

impl Registry {
    /// Registers a factory for `plugin_type`.
    /// Registering the same name again replaces the previous factory.
    pub fn register_plugin_type<F, P>(&mut self, plugin_type: &str, factory: F)
    where
        F: Fn() -> P + 'static,
        P: HypertilePlugin + 'static,
    {
        self.factories.insert(
            plugin_type.to_string(),
            Box::new(move || Box::new(factory())),
        );
    }

    /// Returns registered plugin type names in sorted order.
    pub fn registered_types(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }

    pub fn instances_mut(&mut self) -> &mut HashMap<PaneId, PluginInstance> {
        &mut self.instances
    }

    /// Creates a fresh plugin, calls `on_mount`, and stores it for `pane_id`.
    pub fn spawn_plugin(
        &mut self,
        plugin_type: &str,
        pane_id: PaneId,
        active: bool,
    ) -> Result<(), RegistryError> {
        if self.instances.contains_key(&pane_id) {
            return Err(RegistryError::DuplicatePane(pane_id));
        }
        let mut plugin = self.instantiate_plugin(plugin_type)?;
        plugin.on_mount(PluginContext { pane_id });
        plugin.on_active_change(active);
        self.instances.insert(
            pane_id,
            PluginInstance {
                plugin_type: plugin_type.to_string(),
                plugin,
            },
        );
        Ok(())
    }

    /// Creates a plugin without mounting or storing it.
    pub fn instantiate_plugin(
        &self,
        plugin_type: &str,
    ) -> Result<Box<dyn HypertilePlugin>, RegistryError> {
        let factory = self
            .factories
            .get(plugin_type)
            .ok_or_else(|| RegistryError::UnknownPluginType(plugin_type.to_string()))?;
        Ok(factory())
    }

    /// Calls `on_mount` and stores an existing plugin instance for `pane_id`.
    /// If `pane_id` already has a plugin, the old instance is replaced.
    pub fn mount_plugin_instance(
        &mut self,
        pane_id: PaneId,
        plugin_type: &str,
        mut plugin: Box<dyn HypertilePlugin>,
        active: bool,
    ) {
        plugin.on_mount(PluginContext { pane_id });
        plugin.on_active_change(active);
        self.instances.insert(
            pane_id,
            PluginInstance {
                plugin_type: plugin_type.to_string(),
                plugin,
            },
        );
    }

    pub fn insert_plugin(&mut self, pane_id: PaneId, plugin: PluginInstance) {
        self.instances.insert(pane_id, plugin);
    }

    /// Calls `on_unmount` and removes the plugin for `pane_id`.
    pub fn remove_plugin(&mut self, pane_id: PaneId) -> Result<(), RegistryError> {
        let Some(mut instance) = self.instances.remove(&pane_id) else {
            return Err(RegistryError::MissingPane(pane_id));
        };
        instance.plugin.on_unmount(PluginContext { pane_id });
        Ok(())
    }

    /// Pop the plugin and keep it alive
    pub fn pop_plugin(&mut self, pane_id: PaneId) -> Result<PluginInstance, RegistryError> {
        let Some(instance) = self.instances.remove(&pane_id) else {
            return Err(RegistryError::MissingPane(pane_id));
        };
        Ok(instance)
    }

    /// Returns `true` if a plugin was removed.
    pub fn remove_plugin_if_exists(&mut self, pane_id: PaneId) -> bool {
        self.remove_plugin(pane_id).is_ok()
    }

    /// Unmounts and removes every mounted plugin.
    pub fn clear(&mut self) {
        let pane_ids = self.instances.keys().copied().collect::<Vec<_>>();
        for pane_id in pane_ids {
            let _ = self.remove_plugin(pane_id);
        }
    }

    /// Unmounts any plugin whose pane id is not in `keep`.
    pub fn retain_only(&mut self, keep: &HashSet<PaneId>) {
        let to_remove: Vec<PaneId> = self
            .instances
            .keys()
            .filter(|pane_id| !keep.contains(pane_id))
            .copied()
            .collect();

        for pane_id in to_remove {
            let _ = self.remove_plugin(pane_id);
        }
    }

    pub fn plugin_type_for(&self, pane_id: PaneId) -> Option<&str> {
        self.instances
            .get(&pane_id)
            .map(|instance| instance.plugin_type.as_str())
    }

    pub fn plugin(&self, pane_id: PaneId) -> Option<&dyn HypertilePlugin> {
        self.instances
            .get(&pane_id)
            .map(|instance| instance.plugin.as_ref())
    }

    pub fn plugin_mut(&mut self, pane_id: PaneId) -> Option<&mut (dyn HypertilePlugin + 'static)> {
        self.instances
            .get_mut(&pane_id)
            .map(move |instance| instance.plugin.as_mut())
    }

    pub fn plugin_instance_mut(&mut self, pane_id: PaneId) -> Option<&mut PluginInstance> {
        self.instances.get_mut(&pane_id)
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Forwards `event` to every mounted plugin.
    /// Returns [`EventOutcome::Consumed`] if any plugin consumes it.
    pub fn broadcast_event(&mut self, event: &mut HypertileEvent) -> EventOutcome {
        let mut consumed = false;
        for instance in self.instances.values_mut() {
            if instance.plugin.on_event(event).is_consumed() {
                consumed = true;
            }
        }
        if consumed {
            EventOutcome::Consumed
        } else {
            EventOutcome::Ignored
        }
    }
}
