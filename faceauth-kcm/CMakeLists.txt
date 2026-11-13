# SPDX-License-Identifier: MIT OR Apache-2.0
cmake_minimum_required(VERSION 3.22)

# Version comes from the Cargo workspace via the Makefile.
set(FACEAUTH_VERSION "0.0.0" CACHE STRING "faceauth version (set by the Makefile)")
project(faceauth-kcm VERSION ${FACEAUTH_VERSION} LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(ECM 6.0 REQUIRED NO_MODULE)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)
include(KDECompilerSettings NO_POLICY_SCOPE)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Quick Qml DBus QuickControls2)
# 6.8: KLocalization::setupLocalizedContext (KI18nQml).
find_package(KF6 6.8 REQUIRED COMPONENTS CoreAddons KCMUtils I18n)

add_subdirectory(src)
# Installs po/<lang>/kcm_faceauth.po as the kcm_faceauth catalog; the KCM's
# QML uses the plugin ID as its translation domain.
ki18n_install(po)

install(FILES io.github.gvarsanyi.faceauth.desktop DESTINATION ${KDE_INSTALL_APPDIR})
