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

project('llvm', ['cpp'])

llvm_path = get_option('llvm-path')
if llvm_path != ''
    message('Use local LLVM installation at ' + llvm_path)
    _inc = join_paths(llvm_path, 'include')
    if target_machine.system() == 'windows'
        llvm_c_dylib = join_paths(llvm_path, 'bin', 'LLVM-C.dll')
    elif target_machine.system() != 'linux'
        error('Unsupported target system: ' + target_machine.system())
    endif
else
    message('Will download LLVM releases.')
    if target_machine.system() == 'windows'
        llvm_downloaded = subproject('llvm-downloaded-windows')
        llvm_c_dylib = llvm_downloaded.get_variable('llvm_c_dylib')
    elif target_machine.system() == 'linux'
        # To run this project, it is required to install LLVM. Unlike on the Windows, meson should
        # usually be able to find the LLVM installation.
        error('Unimplemented')
    else
        error('Unsupported target system: ' + target_machine.system())
    endif
    llvm_path = llvm_downloaded.get_variable('path')
    _inc = llvm_downloaded.get_variable('include_directories')
endif


cpp = meson.get_compiler('cpp')

_deps = []
_search = join_paths(llvm_path, 'lib')
_lib_names = [
    'LLVMAArch64AsmParser',
    'LLVMAArch64CodeGen',
    'LLVMAArch64Desc',
    'LLVMAArch64Disassembler',
    'LLVMAArch64Info',
    'LLVMAArch64Utils',
    'LLVMAggressiveInstCombine',
    'LLVMAMDGPUAsmParser',
    'LLVMAMDGPUCodeGen',
    'LLVMAMDGPUDesc',
    'LLVMAMDGPUDisassembler',
    'LLVMAMDGPUInfo',
    'LLVMAMDGPUTargetMCA',
    'LLVMAMDGPUUtils',
    'LLVMAnalysis',
    'LLVMARMAsmParser',
    'LLVMARMCodeGen',
    'LLVMARMDesc',
    'LLVMARMDisassembler',
    'LLVMARMInfo',
    'LLVMARMUtils',
    'LLVMAsmParser',
    'LLVMAsmPrinter',
    'LLVMAVRAsmParser',
    'LLVMAVRCodeGen',
    'LLVMAVRDesc',
    'LLVMAVRDisassembler',
    'LLVMAVRInfo',
    'LLVMBinaryFormat',
    'LLVMBitReader',
    'LLVMBitstreamReader',
    'LLVMBitWriter',
    'LLVMBPFAsmParser',
    'LLVMBPFCodeGen',
    'LLVMBPFDesc',
    'LLVMBPFDisassembler',
    'LLVMBPFInfo',
    'LLVMCFGuard',
    'LLVMCFIVerify',
    'LLVMCodeGen',
    'LLVMCodeGenTypes',
    'LLVMCore',
    'LLVMCoroutines',
    'LLVMCoverage',
    'LLVMDebugInfoBTF',
    'LLVMDebugInfoCodeView',
    'LLVMDebuginfod',
    'LLVMDebugInfoDWARF',
    'LLVMDebugInfoGSYM',
    'LLVMDebugInfoLogicalView',
    'LLVMDebugInfoMSF',
    'LLVMDebugInfoPDB',
    'LLVMDemangle',
    'LLVMDiff',
    'LLVMDlltoolDriver',
    'LLVMDWARFLinker',
    'LLVMDWARFLinkerClassic',
    'LLVMDWARFLinkerParallel',
    'LLVMDWP',
    'LLVMExecutionEngine',
    'LLVMExegesis',
    'LLVMExegesisAArch64',
    'LLVMExegesisMips',
    'LLVMExegesisPowerPC',
    'LLVMExegesisX86',
    'LLVMExtensions',
    'LLVMFileCheck',
    'LLVMFrontendDriver',
    'LLVMFrontendHLSL',
    'LLVMFrontendOffloading',
    'LLVMFrontendOpenACC',
    'LLVMFrontendOpenMP',
    'LLVMFuzzerCLI',
    'LLVMFuzzMutate',
    'LLVMGlobalISel',
    'LLVMHexagonAsmParser',
    'LLVMHexagonCodeGen',
    'LLVMHexagonDesc',
    'LLVMHexagonDisassembler',
    'LLVMHexagonInfo',
    'LLVMHipStdPar',
    'LLVMInstCombine',
    'LLVMInstrumentation',
    'LLVMInterfaceStub',
    'LLVMInterpreter',
    'LLVMipo',
    'LLVMIRPrinter',
    'LLVMIRReader',
    'LLVMJITLink',
    'LLVMLanaiAsmParser',
    'LLVMLanaiCodeGen',
    'LLVMLanaiDesc',
    'LLVMLanaiDisassembler',
    'LLVMLanaiInfo',
    'LLVMLibDriver',
    'LLVMLineEditor',
    'LLVMLinker',
    'LLVMLoongArchAsmParser',
    'LLVMLoongArchCodeGen',
    'LLVMLoongArchDesc',
    'LLVMLoongArchDisassembler',
    'LLVMLoongArchInfo',
    'LLVMLTO',
    'LLVMMC',
    'LLVMMCA',
    'LLVMMCDisassembler',
    'LLVMMCJIT',
    'LLVMMCParser',
    'LLVMMipsAsmParser',
    'LLVMMipsCodeGen',
    'LLVMMipsDesc',
    'LLVMMipsDisassembler',
    'LLVMMipsInfo',
    'LLVMMIRParser',
    'LLVMMSP430AsmParser',
    'LLVMMSP430CodeGen',
    'LLVMMSP430Desc',
    'LLVMMSP430Disassembler',
    'LLVMMSP430Info',
    'LLVMNVPTXCodeGen',
    'LLVMNVPTXDesc',
    'LLVMNVPTXInfo',
    'LLVMObjCARCOpts',
    'LLVMObjCopy',
    'LLVMObject',
    'LLVMObjectYAML',
    'LLVMOption',
    'LLVMOrcDebugging',
    'LLVMOrcJIT',
    'LLVMOrcShared',
    'LLVMOrcTargetProcess',
    'LLVMPasses',
    'LLVMPowerPCAsmParser',
    'LLVMPowerPCCodeGen',
    'LLVMPowerPCDesc',
    'LLVMPowerPCDisassembler',
    'LLVMPowerPCInfo',
    'LLVMProfileData',
    'LLVMRemarks',
    'LLVMRISCVAsmParser',
    'LLVMRISCVCodeGen',
    'LLVMRISCVDesc',
    'LLVMRISCVDisassembler',
    'LLVMRISCVInfo',
    'LLVMRISCVTargetMCA',
    'LLVMRuntimeDyld',
    'LLVMScalarOpts',
    'LLVMSelectionDAG',
    'LLVMSparcAsmParser',
    'LLVMSparcCodeGen',
    'LLVMSparcDesc',
    'LLVMSparcDisassembler',
    'LLVMSparcInfo',
    'LLVMSupport',
    'LLVMSymbolize',
    'LLVMSystemZAsmParser',
    'LLVMSystemZCodeGen',
    'LLVMSystemZDesc',
    'LLVMSystemZDisassembler',
    'LLVMSystemZInfo',
    'LLVMTableGen',
    'LLVMTableGenCommon',
    'LLVMTableGenGlobalISel',
    'LLVMTarget',
    'LLVMTargetParser',
    'LLVMTextAPI',
    'LLVMTextAPIBinaryReader',
    'LLVMTransformUtils',
    'LLVMVEAsmParser',
    'LLVMVECodeGen',
    'LLVMVectorize',
    'LLVMVEDesc',
    'LLVMVEDisassembler',
    'LLVMVEInfo',
    'LLVMWebAssemblyAsmParser',
    'LLVMWebAssemblyCodeGen',
    'LLVMWebAssemblyDesc',
    'LLVMWebAssemblyDisassembler',
    'LLVMWebAssemblyInfo',
    'LLVMWebAssemblyUtils',
    'LLVMWindowsDriver',
    'LLVMWindowsManifest',
    'LLVMX86AsmParser',
    'LLVMX86CodeGen',
    'LLVMX86Desc',
    'LLVMX86Disassembler',
    'LLVMX86Info',
    'LLVMX86TargetMCA',
    'LLVMXCoreCodeGen',
    'LLVMXCoreDesc',
    'LLVMXCoreDisassembler',
    'LLVMXCoreInfo',
    'LLVMXRay',
    'LTO',
    'Remarks',
]
foreach d : _lib_names
    _deps += cpp.find_library(d, dirs: _search)
endforeach

if target_machine.system() == 'windows'
    install_data(llvm_c_dylib, install_dir: 'bin')
endif

_link_args = []
if target_machine.system() == 'linux'
    _link_args = ['-ltinfo']
endif
dep_llvm = declare_dependency(
    include_directories: _inc,
    link_args: _link_args,
    dependencies: _deps,
    version: '18.1.8',
)

has_rtti = false
