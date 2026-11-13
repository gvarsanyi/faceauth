# SPDX-License-Identifier: MIT OR Apache-2.0
set(FACEAUTH_DEFINES
    FACEAUTH_VERSION="${PROJECT_VERSION}"
    TRANSLATION_DOMAIN="kcm_faceauth"
)

# --- KCM plugin ---
# kcmutils_add_qml_kcm builds a MODULE library, installs it to the KCM plugin
# dir, and bundles ui/*.qml into the plugin.
kcmutils_add_qml_kcm(kcm_faceauth
    SOURCES
        FaceAuthDaemon.cpp
        FaceAuthBackend.cpp
        FaceAuthKcm.cpp
        kcm_faceauth.json
)
target_link_libraries(kcm_faceauth PRIVATE
    Qt6::Core
    Qt6::Quick
    Qt6::DBus
    KF6::CoreAddons
    KF6::KCMUtils
    KF6::I18n
)
target_compile_definitions(kcm_faceauth PRIVATE ${FACEAUTH_DEFINES})

# --- Standalone application ---
qt_add_executable(faceauth-gui
    main.cpp
    FaceAuthDaemon.cpp
    FaceAuthBackend.cpp
)
qt_add_qml_module(faceauth-gui
    URI org.faceauth.kcm
    VERSION 1.0
    QML_FILES
        ui/main.qml
        ui/standalone.qml
    NO_PLUGIN
)
# Suppress the "link target does not exist" warning from
# qt6_import_qml_plugins on dynamic builds.
set_target_properties(faceauth-gui PROPERTIES QT_QML_MODULE_NO_IMPORT_SCAN TRUE)
target_link_libraries(faceauth-gui PRIVATE
    Qt6::Core
    Qt6::Gui
    Qt6::Quick
    Qt6::Qml
    Qt6::DBus
    Qt6::QuickControls2
    KF6::CoreAddons
    KF6::I18n
    KF6::I18nQml
)
target_compile_definitions(faceauth-gui PRIVATE ${FACEAUTH_DEFINES})
install(TARGETS faceauth-gui DESTINATION ${KDE_INSTALL_BINDIR})
