package com.hafidz.browserrelay;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.util.AttributeSet;
import android.view.GestureDetector;
import android.view.MotionEvent;
import android.view.View;

/**
 * Fullscreen view that renders the relay's JPEG frames (fit-center)
 * and translates touch into relay input, mirroring app.js:
 * <ul>
 *   <li>single tap → mouse down+up (left click) at frame coords</li>
 *   <li>one-finger drag → wheel scroll (content follows the finger)</li>
 *   <li>long-press → right click</li>
 * </ul>
 * Coordinate mapping is the inverse of the draw transform, the same
 * math as app.js toFrame().
 */
public class FrameView extends View {

    public interface InputSink {
        boolean canDrive();
        void mouse(String action, float x, float y, String button);
        void wheel(float x, float y, float dx, float dy);
        /** A touch landed on the content (used to collapse the edge handle). */
        void onContentDown();
    }

    private Bitmap bitmap;
    private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final RectF dst = new RectF();
    private float scale = 1f;
    private float offX;
    private float offY;
    private InputSink sink;
    private final GestureDetector gestures;

    public FrameView(Context ctx, AttributeSet attrs) {
        super(ctx, attrs);
        setBackgroundColor(Color.BLACK);
        gestures = new GestureDetector(ctx, new GestureDetector.SimpleOnGestureListener() {
            @Override
            public boolean onDown(MotionEvent e) {
                if (sink != null) sink.onContentDown();
                return true;
            }

            @Override
            public boolean onSingleTapUp(MotionEvent e) {
                float[] p = toFrame(e.getX(), e.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    sink.mouse("down", p[0], p[1], "left");
                    sink.mouse("up", p[0], p[1], "left");
                }
                return true;
            }

            @Override
            public boolean onScroll(MotionEvent e1, MotionEvent e2,
                                    float distanceX, float distanceY) {
                float[] p = toFrame(e2.getX(), e2.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    // distance is finger travel; wheel deltas move the
                    // content with the finger (same sign convention).
                    sink.wheel(p[0], p[1], distanceX, distanceY);
                }
                return true;
            }

            @Override
            public void onLongPress(MotionEvent e) {
                float[] p = toFrame(e.getX(), e.getY());
                if (p != null && sink != null && sink.canDrive()) {
                    sink.mouse("down", p[0], p[1], "right");
                    sink.mouse("up", p[0], p[1], "right");
                }
            }
        });
        // Long-press should still fire while a scroll is in progress.
        gestures.setIsLongpressEnabled(true);
    }

    public void setInputSink(InputSink sink) {
        this.sink = sink;
    }

    /** Must be called on the main thread. */
    public void setFrame(Bitmap bmp) {
        Bitmap old = bitmap;
        bitmap = bmp;
        if (old != null && old != bmp) old.recycle();
        invalidate();
    }

    /**
     * View coords → frame pixel coords, or null when there is no
     * frame or the point is outside the drawn image.
     */
    public float[] toFrame(float x, float y) {
        if (bitmap == null || scale <= 0) return null;
        float fx = (x - offX) / scale;
        float fy = (y - offY) / scale;
        if (fx < 0 || fy < 0 || fx > bitmap.getWidth() || fy > bitmap.getHeight()) {
            return null;
        }
        return new float[]{fx, fy};
    }

    @Override
    public boolean onTouchEvent(MotionEvent e) {
        return gestures.onTouchEvent(e);
    }

    @Override
    protected void onDraw(Canvas c) {
        super.onDraw(c);
        if (bitmap == null) return;
        int vw = getWidth();
        int vh = getHeight();
        int bw = bitmap.getWidth();
        int bh = bitmap.getHeight();
        if (vw == 0 || vh == 0 || bw == 0 || bh == 0) return;
        scale = Math.min((float) vw / bw, (float) vh / bh);
        offX = (vw - bw * scale) / 2f;
        offY = (vh - bh * scale) / 2f;
        dst.set(offX, offY, offX + bw * scale, offY + bh * scale);
        c.drawBitmap(bitmap, null, dst, paint);
    }
}
