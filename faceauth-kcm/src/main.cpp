#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickStyle>
#include <QUrl>

#include <KAboutData>
#include <KLocalizedString>
#include <KLocalizedContext>

#include "FaceAuthBackend.h"

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);

    KAboutData about(
        QStringLiteral("faceauth-gui"),
        i18n("Face Authentication"),
        QStringLiteral("0.8.0"),
        i18n("Manage face authentication enrollment and settings"),
        KAboutLicense::GPL_V2,
        i18n("© faceauth contributors")
    );
    KAboutData::setApplicationData(about);
    KLocalizedString::setApplicationDomain("kcm_faceauth");

    // Register FaceAuthBackend as a QML singleton under the org.faceauth.kcm
    // module. The engine takes ownership of the returned object.
    // We use qmlRegisterSingletonType (not QML_SINGLETON) so that the KCM
    // plugin can still create its own FaceAuthBackend as a child QObject of
    // FaceAuthKcm — the two modes never coexist in the same process.
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
    engine.rootContext()->setContextObject(new KLocalizedContext(&engine));

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
