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

# TODO: !!! Handle any possible errors

$ErrorActionPreference = 'stop'

Import-Module Microsoft.PowerShell.Security
$right = [System.Security.AccessControl.FileSystemRights]::FullControl
$controlType = [System.Security.AccessControl.AccessControlType]::Allow
$inheritanceFlags = [System.Security.AccessControl.InheritanceFlags]::ObjectInherit +
[System.Security.AccessControl.InheritanceFlags]::ContainerInherit
$params = @(
    "System.Security.AccessControl.FileSystemAccessRule",
    @(
        "Everyone",
        $right,
        $inheritanceFlags,
        [System.Security.AccessControl.PropagationFlags]::None,
        $controlType
    )
)
$dirRule = New-Object @params
$params = @(
    "System.Security.AccessControl.FileSystemAccessRule",
    @(
        "Everyone",
        $right,
        $controlType
    )
)
$fileRule = New-Object @params

$filesAndFolders = Get-ChildItem "$env:USERPROFILE" -recurse | ForEach-Object { $_.FullName }
$filesAndFolders += "$env:USERPROFILE"
foreach ($fileAndFolder in (Get-ChildItem "$env:GITHUB_WORKSPACE" -recurse |
            ForEach-Object { $_.FullName })) {
    $filesAndFolders += $fileAndFolder
}
$filesAndFolders += "$env:GITHUB_WORKSPACE"

foreach ($fileAndFolder in $filesAndFolders) {
    # Use get-item instead because some of the folders have '[' or ']' character and Powershell
    # throws exception trying to do a get-acl or set-acl on them.
    $item = Get-Item -literalpath $fileAndFolder
    $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
    $rule = $fileRule
    if ($item.PSIsContainer) {
        $rule = $dirRule
    }
    $acl.SetAccessRule($rule)
    [System.IO.FileSystemAclExtensions]::SetAccessControl($item, $acl)
}
