cmake_minimum_required(VERSION 3.22)
project(faceauth-kcm VERSION 0.8.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(ECM 6.0 REQUIRED NO_MODULE)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)
include(KDECompilerSettings NO_POLICY_SCOPE)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Quick Qml Network QuickControls2)
find_package(KF6 6.0 REQUIRED COMPONENTS CoreAddons KCMUtils I18n)

add_subdirectory(src)

# Install app icon into the hicolor theme so the KCM and desktop file pick it up.
install(FILES "${CMAKE_CURRENT_SOURCE_DIR}/../assets/icons/faceauth.svg"
        DESTINATION "${KDE_INSTALL_ICONDIR}/hicolor/scalable/apps")
