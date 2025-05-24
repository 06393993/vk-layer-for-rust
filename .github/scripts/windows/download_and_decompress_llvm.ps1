
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

# TODO: !!! Handle error for decompression and later move item.

param(
    [Parameter(Mandatory = $true)]
    [string]$LlvmInstallPath
)

$ErrorActionPreference = 'stop'

Invoke-WebRequest https://github.com/llvm/llvm-project/releases/download/llvmorg-18.1.8/clang+llvm-18.1.8-x86_64-pc-windows-msvc.tar.xz -OutFile C:\Users\Public\clang+llvm-18.1.8-x86_64-pc-windows-msvc.tar.xz
New-Item -ItemType Directory "$LlvmInstallPath" -Force -ErrorAction SilentlyContinue
python -m tarfile -e C:\Users\Public\clang+llvm-18.1.8-x86_64-pc-windows-msvc.tar.xz "$LlvmInstallPath"
Foreach ($dir in (Get-ChildItem -Path "$LlvmInstallPath" -Directory)) {
    Get-ChildItem -Path $dir | Move-Item -Destination "$LlvmInstallPath" -Force
}
