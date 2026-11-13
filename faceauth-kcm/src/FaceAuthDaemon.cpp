// SPDX-License-Identifier: MIT OR Apache-2.0
#include "FaceAuthDaemon.h"

#include <QDBusError>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QPointer>

namespace FaceAuthDaemon {

bool Result::isError(const char *shortName) const
{
    return errorName == QLatin1String(BUS_NAME) + QLatin1String(".Error.") + QLatin1String(shortName);
}

QDBusConnection bus()
{
    return qEnvironmentVariable("FACEAUTH_BUS") == QLatin1String("session") ? QDBusConnection::sessionBus()
                                                                            : QDBusConnection::systemBus();
}

static bool isTransportError(QDBusError::ErrorType type)
{
    switch (type) {
    case QDBusError::ServiceUnknown:
    case QDBusError::NoReply:
    case QDBusError::Timeout:
    case QDBusError::TimedOut:
    case QDBusError::Disconnected:
    case QDBusError::NoServer:
    case QDBusError::UnknownObject:
    case QDBusError::UnknownInterface:
        return true;
    default:
        return false;
    }
}

void call(const QString &method,
          const QVariantList &args,
          QObject *context,
          Callback callback,
          int timeoutMs,
          bool interactive)
{
    QDBusMessage msg = QDBusMessage::createMethodCall(QLatin1String(BUS_NAME),
                                                      QLatin1String(OBJECT_PATH),
                                                      QLatin1String(INTERFACE),
                                                      method);
    msg.setArguments(args);
    msg.setInteractiveAuthorizationAllowed(interactive);

    QDBusConnection connection = bus();
    if (!connection.isConnected()) {
        Result r;
        r.unavailable = true;
        r.errorMessage = QStringLiteral("system bus not available");
        callback(r);
        return;
    }

    auto *watcher = new QDBusPendingCallWatcher(connection.asyncCall(msg, timeoutMs), context);
    QObject::connect(watcher, &QDBusPendingCallWatcher::finished, context,
                     [callback = std::move(callback)](QDBusPendingCallWatcher *w) {
                         const QDBusMessage reply = w->reply();
                         Result r;
                         if (reply.type() == QDBusMessage::ReplyMessage) {
                             r.ok = true;
                             r.values = reply.arguments();
                         } else {
                             const QDBusError err = w->error();
                             r.unavailable = isTransportError(err.type());
                             r.errorName = err.name();
                             r.errorMessage = err.message();
                         }
                         w->deleteLater();
                         callback(r);
                     });
}

} // namespace FaceAuthDaemon
