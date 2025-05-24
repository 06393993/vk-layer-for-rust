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

[System.Diagnostics.CodeAnalysis.SuppressMessageAttribute(
    "PSAvoidUsingConvertToSecureStringWithPlainText",
    "",
    Justification = "A test account created with plain texts."
)]
param(
    [Parameter(Mandatory = $true)]
    [string]$LlvmInstallPath
)

$ErrorActionPreference = 'stop'

$credArgList = @("testuser", (ConvertTo-SecureString -String "i23456@B" -AsPlainText -Force))
$credentials = New-Object System.Management.Automation.PSCredential -ArgumentList $credArgList
$params = @{
    FilePath = "cargo.exe"
    ArgumentList = @(
        "make",
        "ci-e2e-test",
        "--llvm-path",
        $LlvmInstallPath
    )
    NoNewWindow = $true
    PassThru = $true
    Credential = $credentials
}
$proc = Start-Process @params
$proc.WaitForExit()
exit $proc.ExitCode
