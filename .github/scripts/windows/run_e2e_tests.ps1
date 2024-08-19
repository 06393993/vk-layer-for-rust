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

$ErrorActionPreference = 'stop'

param(
    [Parameter(Mandatory = $true)]
    [string]$LlvmInstallPath
)

$credentials = New-Object System.Management.Automation.PSCredential -ArgumentList @("testuser", (ConvertTo-SecureString -String "i23456@B" -AsPlainText -Force))
$proc = Start-Process -FilePath cargo.exe -ArgumentList "make", "ci-e2e-test", "--llvm-path", "$LlvmInstallPath" -NoNewWindow -PassThru -Credential $credentials
$proc.WaitForExit()
exit $proc.ExitCode
