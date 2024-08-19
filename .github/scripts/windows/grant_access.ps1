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
$Right = [System.Security.AccessControl.FileSystemRights]::FullControl
$ControlType = [System.Security.AccessControl.AccessControlType]::Allow
$Inheritance = [System.Security.AccessControl.InheritanceFlags]::ObjectInherit + [System.Security.AccessControl.InheritanceFlags]::ContainerInherit
$Propagation = [System.Security.AccessControl.PropagationFlags]::None
$DirRule = New-Object System.Security.AccessControl.FileSystemAccessRule "Everyone", $Right, $Inheritance, $Propagation, $ControlType
$FileRule = New-Object System.Security.AccessControl.FileSystemAccessRule "Everyone", $Right, $ControlType

$FilesAndFolders = Get-ChildItem "$env:USERPROFILE" -recurse | ForEach-Object { $_.FullName }
$FilesAndFolders += "$env:USERPROFILE"
foreach ($FileAndFolder in (Get-ChildItem "$env:GITHUB_WORKSPACE" -recurse | ForEach-Object { $_.FullName })) {
    $FilesAndFolders += $FileAndFolder
}
$FilesAndFolders += "$env:GITHUB_WORKSPACE"

foreach ($FileAndFolder in $FilesAndFolders) {
    #using get-item instead because some of the folders have '[' or ']' character and Powershell throws exception trying to do a get-acl or set-acl on them.
    $Item = Get-Item -literalpath $FileAndFolder
    $Acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($Item)
    $Rule = $FileRule
    if ($Item.PSIsContainer) {
        $Rule = $DirRule
    }
    $Acl.SetAccessRule($Rule)
    [System.IO.FileSystemAclExtensions]::SetAccessControl($Item, $Acl)
}
