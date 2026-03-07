# --- faceauth-notify path ---
# Prefer /usr/libexec (Fedora/Arch); fall back to /usr/lib (Debian/Ubuntu).
if(EXISTS /usr/libexec)
    set(FACEAUTH_LIBEXEC_DIR_DEFAULT /usr/libexec)
else()
    set(FACEAUTH_LIBEXEC_DIR_DEFAULT /usr/lib)
endif()
set(FACEAUTH_LIBEXEC_DIR "${FACEAUTH_LIBEXEC_DIR_DEFAULT}" CACHE PATH
    "Directory where faceauth helper binaries (faceauth-notify) are installed")

# --- KCM plugin ---
# kcmutils_add_qml_kcm creates a MODULE lib, installs it to the KCM plugin dir,
# and bundles all ui/*.qml into the plugin's QRC via qt_add_qml_module.
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
    Qt6::Network
    KF6::CoreAddons
    KF6::KCMUtils
    KF6::I18n
)
target_compile_definitions(kcm_faceauth PRIVATE
    FACEAUTH_LIBEXEC_DIR="${FACEAUTH_LIBEXEC_DIR}"
)

# --- Standalone binary ---
# FaceAuthKcm.cpp is intentionally NOT included here.
qt_add_executable(faceauth-gui
    main.cpp
    FaceAuthDaemon.cpp
    FaceAuthBackend.cpp
)

# qt_add_qml_module embeds QML files as resources under
# qrc:/qt/qml/org/kde/faceauth/ and registers the module so that
# "import org.faceauth.kcm 1.0" resolves inside the binary.
qt_add_qml_module(faceauth-gui
    URI org.faceauth.kcm
    VERSION 1.0
    QML_FILES
        ui/main.qml
        ui/standalone.qml
    NO_PLUGIN
)
# Suppress the spurious "link target does not exist" warning from
# qt6_import_qml_plugins on dynamic builds.
set_target_properties(faceauth-gui PROPERTIES QT_QML_MODULE_NO_IMPORT_SCAN TRUE)

target_link_libraries(faceauth-gui PRIVATE
    Qt6::Core
    Qt6::Gui
    Qt6::Quick
    Qt6::Qml
    Qt6::Network
    Qt6::QuickControls2
    KF6::CoreAddons
    KF6::I18n
)
target_compile_definitions(faceauth-gui PRIVATE
    FACEAUTH_LIBEXEC_DIR="${FACEAUTH_LIBEXEC_DIR}"
)
install(TARGETS faceauth-gui DESTINATION ${KDE_INSTALL_BINDIR})
