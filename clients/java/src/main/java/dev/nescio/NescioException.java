package dev.nescio;

/** Thrown for any non-2xx response; carries the HTTP status and server message. */
public final class NescioException extends RuntimeException {

    private final int status;

    public NescioException(int status, String message) {
        super(message);
        this.status = status;
    }

    /** HTTP status code (0 if the request never reached the server). */
    public int status() {
        return status;
    }
}
