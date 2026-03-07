#include "FaceAuthDaemon.h"

#include <QJsonDocument>
#include <QJsonArray>

// --- DaemonRequest ---

DaemonRequest::DaemonRequest(QByteArray requestJson,
                             Callback callback,
                             int timeoutMs,
                             QObject *parent)
    : QObject(parent)
    , m_requestJson(std::move(requestJson))
    , m_callback(std::move(callback))
{
    m_socket = new QLocalSocket(this);

    connect(m_socket, &QLocalSocket::connected,
            this, &DaemonRequest::onConnected);
    connect(m_socket, &QLocalSocket::readyRead,
            this, &DaemonRequest::onReadyRead);
    connect(m_socket, &QLocalSocket::errorOccurred,
            this, &DaemonRequest::onError);

    auto *timer = new QTimer(this);
    timer->setSingleShot(true);
    timer->setInterval(timeoutMs);
    connect(timer, &QTimer::timeout, this, &DaemonRequest::onTimeout);
    timer->start();

    m_socket->connectToServer(QString::fromLatin1(FaceAuthDaemon::SOCKET_PATH));
}

void DaemonRequest::onConnected()
{
    // Protocol: one JSON line (terminated by '\n'), then read one JSON line back.
    QByteArray payload = m_requestJson;
    payload.append('\n');
    m_socket->write(payload);
}

void DaemonRequest::onReadyRead()
{
    m_buffer.append(m_socket->readAll());
    const int nl = m_buffer.indexOf('\n');
    if (nl < 0)
        return; // wait for the full response line

    const QByteArray line = m_buffer.left(nl).trimmed();
    const QJsonDocument doc = QJsonDocument::fromJson(line);
    if (doc.isNull()) {
        finish(false, QStringLiteral("malformed JSON from daemon"), {});
        return;
    }

    const QJsonObject obj = doc.object();
    const QString status = obj.value(QLatin1String("status")).toString();

    if (status == QLatin1String("Ok")) {
        finish(true, {}, {});
    } else if (status == QLatin1String("Err")) {
        finish(false, obj.value(QLatin1String("message")).toString(), {});
    } else if (status == QLatin1String("Model")) {
        finish(true, {}, obj.value(QLatin1String("json")).toString());
    } else if (status == QLatin1String("Encoding")) {
        const QJsonArray arr = obj.value(QLatin1String("data")).toArray();
        QVariantList data;
        data.reserve(arr.size());
        for (const auto &v : arr)
            data.append(v.toDouble());
        finish(true, {}, data);
    } else if (status == QLatin1String("Cameras")) {
        const QJsonArray arr = obj.value(QLatin1String("cameras")).toArray();
        QVariantList cameras;
        cameras.reserve(arr.size());
        for (const auto &v : arr) {
            const QJsonObject cam = v.toObject();
            const int idx = cam.value(QLatin1String("index")).toInt();
            const QString rawName = cam.value(QLatin1String("name")).toString();
            const QString displayName = rawName.isEmpty()
                ? QStringLiteral("/dev/video%1").arg(idx)
                : QStringLiteral("/dev/video%1 %2").arg(idx).arg(rawName);
            QVariantMap entry;
            entry[QStringLiteral("index")]       = idx;
            entry[QStringLiteral("name")]        = displayName;
            entry[QStringLiteral("suitability")] = cam.value(QLatin1String("suitability")).toInt();
            cameras.append(entry);
        }
        finish(true, {}, cameras);
    } else if (status == QLatin1String("Services")) {
        const QJsonArray arr = obj.value(QLatin1String("services")).toArray();
        QVariantList services;
        services.reserve(arr.size());
        for (const auto &v : arr) {
            const QJsonObject svc = v.toObject();
            QVariantMap entry;
            entry[QStringLiteral("name")]    = svc.value(QLatin1String("name")).toString();
            entry[QStringLiteral("allowed")] = svc.value(QLatin1String("allowed")).toBool();
            services.append(entry);
        }
        finish(true, {}, services);
    } else {
        finish(false, QStringLiteral("unexpected status '%1' from daemon").arg(status), {});
    }
}

void DaemonRequest::onError(QLocalSocket::LocalSocketError)
{
    finish(false, m_socket->errorString(), {}, /*connectionError=*/true);
}

void DaemonRequest::onTimeout()
{
    finish(false, QStringLiteral("daemon request timed out"), {}, /*connectionError=*/true);
}

void DaemonRequest::finish(bool ok, const QString &error, const QVariant &data, bool connectionError)
{
    if (m_done)
        return;
    m_done = true;
    m_socket->abort();
    m_callback(ok, error, data, connectionError);
    deleteLater();
}

// --- FaceAuthDaemon namespace ---

void FaceAuthDaemon::send(QJsonObject request,
                          DaemonRequest::Callback callback,
                          int timeoutMs,
                          QObject *parent)
{
    const QByteArray json = QJsonDocument(request).toJson(QJsonDocument::Compact);
    // DaemonRequest self-destructs; parent keeps it alive until then.
    new DaemonRequest(json, std::move(callback), timeoutMs, parent);
}
