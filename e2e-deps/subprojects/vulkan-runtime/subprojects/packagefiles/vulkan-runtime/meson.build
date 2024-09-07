# Copyright 2024 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

project('vulkan-runtime', ['cpp'])

if target_machine.system() == 'windows'
    install_data(join_paths(meson.current_source_dir(), 'x64', 'vulkan-1.dll'), install_dir: 'bin')
    install_data(
        join_paths(meson.current_source_dir(), 'x64', 'vulkaninfo.exe'),
        install_dir: 'bin',
    )
elif target_machine.system() == 'linux'
    install_subdir(
        join_paths(meson.current_source_dir(), '1.3.290.0', 'x86_64'),
        strip_directory: true,
        install_dir: '.',
    )
else
    error('Unsupported target system: ' + target_machine.system())
endif
