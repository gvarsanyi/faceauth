// SPDX-License-Identifier: MIT OR Apache-2.0
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickStyle>
#include <QUrl>

#include <KAboutData>
#include <kcoreaddons_version.h>
#include <KLocalizedQmlContext>
#include <KLocalizedString>

#include "FaceAuthBackend.h"

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);

    KAboutData about(
        QStringLiteral("faceauth-gui"),
        i18n("Face Authentication"),
        QStringLiteral(FACEAUTH_VERSION),
        i18n("Enroll your face and choose where face authentication is used"),
        KAboutLicense::MIT,
        i18n("Copyright (C) Greg Varsanyi")
    );
#if KCOREADDONS_VERSION >= QT_VERSION_CHECK(6, 9, 0)
    about.addLicense(KAboutLicense::Apache_V2);
#endif
    about.setHomepage(QStringLiteral("https://github.com/gvarsanyi/faceauth"));
    about.setDesktopFileName(QStringLiteral("io.github.gvarsanyi.faceauth"));
    KAboutData::setApplicationData(about);

    // Register FaceAuthBackend as a QML singleton under the org.faceauth.kcm
    // module. The engine takes ownership of the returned object.
    // We use qmlRegisterSingletonType (not QML_SINGLETON) so that the KCM
    // plugin can still create its own FaceAuthBackend as a child QObject of
    // FaceAuthKcm: the two modes never coexist in the same process.
    qmlRegisterSingletonType<FaceAuthBackend>(
        "org.faceauth.kcm", 1, 0, "Backend",
        [](QQmlEngine *, QJSEngine *) -> QObject * {
            return new FaceAuthBackend();
        });

    // Use the org.kde.desktop style when available (provides Breeze-themed
    // QQC2 controls). Falls back to the platform default if not installed.
    if (qEnvironmentVariableIsEmpty("QT_QUICK_CONTROLS_STYLE"))
        QQuickStyle::setStyle(QStringLiteral("org.kde.desktop"));

    QQmlApplicationEngine engine;
    KLocalization::setupLocalizedContext(&engine);

    const QUrl url(QStringLiteral("qrc:/org/faceauth/kcm/ui/standalone.qml"));
    QObject::connect(
        &engine, &QQmlApplicationEngine::objectCreationFailed,
        &app, [](const QUrl &failedUrl) {
            qWarning("Failed to create root object from %s",
                     qPrintable(failedUrl.toString()));
            QCoreApplication::exit(1);
        },
        Qt::QueuedConnection);

    engine.load(url);

    return app.exec();
}
