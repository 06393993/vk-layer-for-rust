// Copyright 2023 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    collections::BTreeMap,
    ptr::null,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use ash::vk;
use mockall::automock;
use once_cell::sync::Lazy;
use vulkan_layer::{
    test_utils::{LayerManifestExt, MockDeviceHooks, MockInstanceHooks},
    DeviceInfo, Global, InstanceInfo, Layer, LayerManifest, LayerResult, LayerVulkanCommand,
    StubGlobalHooks,
};

pub mod utils;

use utils::{ArcDelInstanceContextExt, DeviceContext, InstanceContext, InstanceCreateInfoExt};

#[automock]
trait HookedCommands {
    fn layer_hooked_instance_commands(
        &self,
        instance_info: &TestInstanceInfo,
    ) -> Box<dyn Iterator<Item = LayerVulkanCommand>>;

    // Need the explicit lifetime to make automock work.
    #[allow(clippy::needless_lifetimes)]
    fn layer_hooked_device_commands<'a>(
        &self,
        instance_info: &TestInstanceInfo,
        device_info: Option<&'a TestDeviceInfo>,
    ) -> Box<dyn Iterator<Item = LayerVulkanCommand>>;

    fn instance_info_hooked_commands() -> &'static [LayerVulkanCommand];

    fn device_info_hooked_commands() -> &'static [LayerVulkanCommand];
}

#[derive(Default)]
struct TestInstanceInfo(Mutex<MockInstanceHooks>);

impl InstanceInfo for TestInstanceInfo {
    type HooksType = MockInstanceHooks;
    type HooksRefType<'a> = MutexGuard<'a, MockInstanceHooks>;

    fn hooked_commands() -> &'static [vulkan_layer::LayerVulkanCommand] {
        MockHookedCommands::instance_info_hooked_commands()
    }

    fn hooks(&self) -> Self::HooksRefType<'_> {
        self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct TestDeviceInfo(Mutex<MockDeviceHooks>);

impl DeviceInfo for TestDeviceInfo {
    type HooksType = MockDeviceHooks;
    type HooksRefType<'a> = MutexGuard<'a, MockDeviceHooks>;

    fn hooked_commands() -> &'static [vulkan_layer::LayerVulkanCommand] {
        MockHookedCommands::device_info_hooked_commands()
    }

    fn hooks(&self) -> Self::HooksRefType<'_> {
        self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct TestLayer {
    global_hooks_info: StubGlobalHooks,
    mock: Mutex<MockHookedCommands>,
    instance_map: Mutex<BTreeMap<vk::Instance, Weak<TestInstanceInfo>>>,
    device_map: Mutex<BTreeMap<vk::Device, Weak<TestDeviceInfo>>>,
}

impl TestLayer {
    fn get_instance_info(&self, instance: vk::Instance) -> Option<Arc<TestInstanceInfo>> {
        self.instance_map
            .lock()
            .unwrap()
            .get(&instance)
            .and_then(Weak::upgrade)
    }

    fn get_device_info(&self, device: vk::Device) -> Option<Arc<TestDeviceInfo>> {
        self.device_map
            .lock()
            .unwrap()
            .get(&device)
            .and_then(Weak::upgrade)
    }
}

impl Layer for TestLayer {
    type GlobalHooksInfo = StubGlobalHooks;
    type InstanceInfo = TestInstanceInfo;
    type DeviceInfo = TestDeviceInfo;
    type InstanceInfoContainer = Arc<Self::InstanceInfo>;
    type DeviceInfoContainer = Arc<Self::DeviceInfo>;

    fn manifest() -> LayerManifest {
        LayerManifest::test_default()
    }

    fn global_instance() -> &'static vulkan_layer::Global<Self> {
        static GLOBAL: Lazy<Global<TestLayer>> = Lazy::new(Default::default);
        &GLOBAL
    }

    fn global_hooks_info(&self) -> &Self::GlobalHooksInfo {
        &self.global_hooks_info
    }

    fn create_instance_info(
        &self,
        _: &vk::InstanceCreateInfo,
        _: Option<&vk::AllocationCallbacks>,
        instance: std::sync::Arc<ash::Instance>,
        _next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    ) -> Self::InstanceInfoContainer {
        let instance_info = Arc::<TestInstanceInfo>::default();
        self.instance_map
            .lock()
            .unwrap()
            .insert(instance.handle(), Arc::downgrade(&instance_info));
        instance_info
    }

    fn create_device_info(
        &self,
        _: vk::PhysicalDevice,
        _: &vk::DeviceCreateInfo,
        _: Option<&vk::AllocationCallbacks>,
        device: std::sync::Arc<ash::Device>,
        _next_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    ) -> Self::DeviceInfoContainer {
        let device_info = Arc::<TestDeviceInfo>::default();
        self.device_map
            .lock()
            .unwrap()
            .insert(device.handle(), Arc::downgrade(&device_info));
        device_info
    }

    fn hooked_instance_commands(
        &self,
        instance_info: &Self::InstanceInfo,
    ) -> Box<dyn Iterator<Item = LayerVulkanCommand>> {
        self.mock
            .lock()
            .unwrap()
            .layer_hooked_instance_commands(instance_info)
    }

    fn hooked_device_commands(
        &self,
        instance_info: &Self::InstanceInfo,
        device_info: Option<&Self::DeviceInfo>,
    ) -> Box<dyn Iterator<Item = LayerVulkanCommand>> {
        self.mock
            .lock()
            .unwrap()
            .layer_hooked_device_commands(instance_info, device_info)
    }
}

mod get_instance_proc_addr {
    use super::*;

    #[test]
    fn test_layer_hooked_instance_commands_should_take_priority() {
        let ctx = MockHookedCommands::instance_info_hooked_commands_context();
        // InstanceInfo::hooked_commands returns 2 commands.
        ctx.expect().return_const(
            [
                LayerVulkanCommand::DestroySurfaceKhr,
                LayerVulkanCommand::DestroyDebugUtilsMessengerExt,
            ]
            .as_slice(),
        );
        let ctx = MockHookedCommands::device_info_hooked_commands_context();
        ctx.expect().return_const([].as_slice());
        {
            let mut mock = TestLayer::global_instance().layer_info.mock.lock().unwrap();
            // Layer::hooked_instance_commands only returns one of 2 commands.
            mock.expect_layer_hooked_instance_commands().returning(|_| {
                Box::new([LayerVulkanCommand::DestroySurfaceKhr].into_iter())
                    as Box<dyn Iterator<Item = LayerVulkanCommand>>
            });
            mock.expect_layer_hooked_device_commands()
                .withf(|_, device_info| device_info.is_none())
                .returning(|_, _| {
                    Box::new([].into_iter()) as Box<dyn Iterator<Item = LayerVulkanCommand>>
                });
        }

        let enabled_instance_extensions = [
            vk::KhrSurfaceFn::name().as_ptr(),
            vk::ExtDebugUtilsFn::name().as_ptr(),
        ];
        let instance_ctx = vk::InstanceCreateInfo::builder()
            .enabled_extension_names(&enabled_instance_extensions)
            .default_instance::<(TestLayer,)>();
        let InstanceContext {
            instance, entry, ..
        } = instance_ctx.as_ref();
        let instance_info = TestLayer::global_instance()
            .layer_info
            .get_instance_info(instance.handle())
            .unwrap();
        instance_info
            .hooks()
            .expect_destroy_debug_utils_messenger_ext()
            .never();
        instance_info
            .hooks()
            .expect_destroy_surface_khr()
            .once()
            .return_const(LayerResult::Unhandled);

        let destroy_surface = unsafe {
            entry.get_instance_proc_addr(
                instance.handle(),
                b"vkDestroySurfaceKHR\0".as_ptr() as *const i8,
            )
        }
        .unwrap();
        let destroy_surface: vk::PFN_vkDestroySurfaceKHR =
            unsafe { std::mem::transmute(destroy_surface) };
        let destroy_debug_utils_messenger = unsafe {
            entry.get_instance_proc_addr(
                instance.handle(),
                b"vkDestroyDebugUtilsMessengerEXT\0".as_ptr() as *const i8,
            )
        }
        .unwrap();
        let destroy_debug_utils_messenger: vk::PFN_vkDestroyDebugUtilsMessengerEXT =
            unsafe { std::mem::transmute(destroy_debug_utils_messenger) };
        unsafe { destroy_surface(instance.handle(), vk::SurfaceKHR::null(), null()) };
        unsafe {
            destroy_debug_utils_messenger(
                instance.handle(),
                vk::DebugUtilsMessengerEXT::null(),
                null(),
            )
        };
    }

    #[test]
    fn test_layer_hooked_device_commands_should_take_priority() {
        let ctx = MockHookedCommands::instance_info_hooked_commands_context();
        ctx.expect().return_const([].as_slice());
        // DeviceInfo::hooked_commands returns 2 commands.
        let ctx = MockHookedCommands::device_info_hooked_commands_context();
        ctx.expect().return_const(
            [
                LayerVulkanCommand::DestroyImage,
                LayerVulkanCommand::FreeMemory,
            ]
            .as_slice(),
        );

        {
            let mut mock = TestLayer::global_instance().layer_info.mock.lock().unwrap();
            mock.expect_layer_hooked_instance_commands().returning(|_| {
                Box::new([].into_iter()) as Box<dyn Iterator<Item = LayerVulkanCommand>>
            });
            // Layer::hooked_device_commands only returns one of 2 commands.
            mock.expect_layer_hooked_device_commands()
                .returning(|_, _| {
                    Box::new([LayerVulkanCommand::DestroyImage].into_iter())
                        as Box<dyn Iterator<Item = LayerVulkanCommand>>
                });
        }

        let device_ctx = vk::InstanceCreateInfo::builder()
            .default_instance::<(TestLayer,)>()
            .default_device()
            .unwrap();
        let DeviceContext {
            device,
            instance_context,
            ..
        } = device_ctx.as_ref();

        let device_info = TestLayer::global_instance()
            .layer_info
            .get_device_info(device.handle())
            .unwrap();
        device_info
            .hooks()
            .expect_destroy_image()
            .once()
            .return_const(LayerResult::Unhandled);
        device_info.hooks().expect_free_memory().never();

        let InstanceContext {
            entry, instance, ..
        } = instance_context.as_ref();
        let destroy_image = unsafe {
            entry.get_instance_proc_addr(
                instance.handle(),
                b"vkDestroyImage\0".as_ptr() as *const i8,
            )
        }
        .unwrap();
        let destroy_image: vk::PFN_vkDestroyImage = unsafe { std::mem::transmute(destroy_image) };
        unsafe { destroy_image(device.handle(), vk::Image::null(), null()) };
        let free_memory = unsafe {
            entry.get_instance_proc_addr(instance.handle(), b"vkFreeMemory\0".as_ptr() as *const i8)
        }
        .unwrap();
        let free_memory: vk::PFN_vkFreeMemory = unsafe { std::mem::transmute(free_memory) };
        unsafe { free_memory(device.handle(), vk::DeviceMemory::null(), null()) };
    }
}

mod get_device_proc_addr {
    use super::*;

    #[test]
    fn test_layer_hooked_device_commands_should_take_priority() {
        let ctx = MockHookedCommands::instance_info_hooked_commands_context();
        ctx.expect().return_const([].as_slice());
        // DeviceInfo::hooked_commands returns 2 commands.
        let ctx = MockHookedCommands::device_info_hooked_commands_context();
        ctx.expect().return_const(
            [
                LayerVulkanCommand::DestroyImage,
                LayerVulkanCommand::FreeMemory,
            ]
            .as_slice(),
        );

        {
            let mut mock = TestLayer::global_instance().layer_info.mock.lock().unwrap();
            mock.expect_layer_hooked_instance_commands().returning(|_| {
                Box::new([].into_iter()) as Box<dyn Iterator<Item = LayerVulkanCommand>>
            });
            // Layer::hooked_device_commands only returns one of 2 commands.
            mock.expect_layer_hooked_device_commands()
                .returning(|_, _| {
                    Box::new([LayerVulkanCommand::DestroyImage].into_iter())
                        as Box<dyn Iterator<Item = LayerVulkanCommand>>
                });
        }

        let device_ctx = vk::InstanceCreateInfo::builder()
            .default_instance::<(TestLayer,)>()
            .default_device()
            .unwrap();
        let DeviceContext { device, .. } = device_ctx.as_ref();

        let device_info = TestLayer::global_instance()
            .layer_info
            .get_device_info(device.handle())
            .unwrap();
        device_info
            .hooks()
            .expect_destroy_image()
            .once()
            .return_const(LayerResult::Unhandled);
        device_info.hooks().expect_free_memory().never();

        unsafe { device.destroy_image(vk::Image::null(), None) };
        unsafe { device.free_memory(vk::DeviceMemory::null(), None) };
    }
}
